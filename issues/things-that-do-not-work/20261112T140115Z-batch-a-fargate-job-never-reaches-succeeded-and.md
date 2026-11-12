Issue-Source: trace-sweep-divergence
Issue-Trace: batch-fargate-job-runs-to-succeeded
Issue-Step: task#17
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: batch-fargate-job-runs-to-succeeded
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch|DescribeJobs|2xx->2xx|$.jobs[*].attempts[*].container.exitCode
Issue-Key: batch|DescribeJobs|2xx->2xx|$.jobs[*].attempts[*].container.networkInterfaces
Issue-Key: batch|DescribeJobs|2xx->2xx|$.jobs[*].attempts[*].container.taskArn
Issue-Key: batch|DescribeJobs|2xx->2xx|$.jobs[*].attempts[*].startedAt
Issue-Key: batch|DescribeJobs|2xx->2xx|$.jobs[*].attempts[*].statusReason
Issue-Key: batch|DescribeJobs|2xx->2xx|$.jobs[*].attempts[*].stoppedAt
Issue-Related: 20261109T161717Z-batch-3-net-new-causes-tag-untag-204.md
Issue-Commit: a6732131b4577aead0e874dcd27f439fc83f19d6

# batch: a FARGATE job never reaches SUCCEEDED and DescribeJobs omits JobDetail's attempt, timestamps, empty collections and applied defaults

> _Note: this issue was identified against the code base at commit `a6732131b4577aead0e874dcd27f439fc83f19d6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeJobs

## Description
One net-new root cause in the batch emulator, found by replaying the wire fixture corpus (trace set simparity-wire-aws-v1).

Cause 1 of 1: a container job submitted with platformCapabilities FARGATE to a healthy job queue settles FAILED with an emulator-authored statusReason, and DescribeJobs reports no attempt, no timestamps and none of JobDetail's empty collections or applied defaults.

AWS: a Batch job submitted to a valid job queue advances through the modeled JobStatus ladder (Smithy: batch JobStatus = SUBMITTED, PENDING, RUNNABLE, STARTING, RUNNING, SUCCEEDED, FAILED) and reaches SUCCEEDED when its essential container exits 0. DescribeJobs (Smithy: DescribeJobs, http POST /v1/describejobs, code 200, output DescribeJobsResponse.jobs: JobDetailList; errors ClientException 400 / ServerException 500) then reports, for that job:
  - status SUCCEEDED and statusReason the fixed AWS sentence `Essential container in task exited` (that text is entirely fixed — it carries no variable parts),
  - startedAt and stoppedAt present (Smithy: JobDetail.startedAt / JobDetail.stoppedAt, epoch-millisecond Longs — the concrete values are per-run and are NOT part of the requirement, only their presence),
  - attempts containing one AttemptDetail (Smithy: JobDetail.attempts -> AttemptDetails; AttemptDetail.{container: AttemptContainerDetail, startedAt, stoppedAt, statusReason}) whose container carries taskArn, exitCode 0, logStreamName and networkInterfaces[].{attachmentId, privateIpv4Address}, and whose statusReason is the same fixed `Essential container in task exited`,
  - the empty collections and applied defaults AWS always materializes on a described job: dependsOn [], parameters {}, eksAttempts [], container.volumes [], container.environment [], container.mountPoints [], container.ulimits [], container.secrets [], container.exitCode 0, container.taskArn, container.networkInterfaces and container.fargatePlatformConfiguration {platformVersion: "LATEST"} (the Fargate default applied at registration).

Emulator: the job settles FAILED instead of SUCCEEDED, and statusReason is the emulator-authored sentence "This emulator does not run containers, so the job's container was never started" — text AWS emits for no status, on either the JobDetail or the attempt. attempts[0].startedAt, attempts[0].stoppedAt, attempts[0].container.exitCode, attempts[0].container.networkInterfaces, JobDetail.startedAt, JobDetail.stoppedAt, dependsOn, parameters, eksAttempts and container.{volumes, environment, mountPoints, ulimits, secrets, exitCode, networkInterfaces, fargatePlatformConfiguration} are all reported `missing` (None) where AWS sends a value, a list or a map.

Why: every client that waits for a Batch job to finish — the SDK job waiters, CI drivers, Step Functions' and CloudFormation's Batch integrations, and any poller written against the documented status ladder — concludes the workload failed, and is then handed a failure reason that matches no AWS statusReason, so failure-classification logic cannot branch on it. Code that reads attempts[0].container.exitCode / taskArn / logStreamName to locate the ECS task or its /aws/batch/job log stream dereferences absent members, and code that iterates container.environment, container.ulimits or parameters without a null guard breaks on None where AWS guarantees an (possibly empty) list or map.

Why it is filed here: the replay reports this step as owned by NO open issue. Open issue 20261109T161717Z-batch-3-net-new-causes-tag-untag-204.md describes this same cause as its "Cause 3 of 3", but its replay pointer names the fixture `batch-fargate-job-runs-to-succeeded` with no step, so this fixture#step binding is not carried by any filed issue and a fix closing that issue would never re-replay it. Open issues 20261106T090000Z-batch-s3-a-placed-job-s-container-is-never-run.md, 20261107T210820Z-batch-the-job-data-plane-never-executes-a.md and 20261107T153156Z-batch-logs-a-placed-batch-job-ends-failed.md describe the same family on other fixtures and likewise do not carry this key. Close this one against this fixture/step even if those are already fixed.

Fix: the SubmitJob / job-lifecycle state machine in the batch crate for the FARGATE platform capability — a placed job must advance to SUCCEEDED with a synthesized attempt (task arn, exit code 0, log stream name, network interface) and startedAt/stoppedAt timestamps, and set the AWS statusReason text — plus the DescribeJobs response serializer, which must emit JobDetail's empty collections and the registration-applied defaults instead of omitting those members.

Traces: batch-fargate-job-runs-to-succeeded#task#17.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up and serving; score 84.8%, 24 mismatched of 165 steps, 16 divergence clusters).

DescribeJobs, expected status 200 / actual 200, fixture batch-fargate-job-runs-to-succeeded step task#17 (cluster affected_count 1):
  $.jobs[0].status: value_mismatch (expected 'SUCCEEDED', got 'FAILED')
  $.jobs[0].statusReason: value_mismatch (expected 'Essential container in task exited', got "This emulator does not run containers, so the job's container was never started")
  $.jobs[0].attempts[0].statusReason: value_mismatch (expected 'Essential container in task exited', got "This emulator does not run containers, so the job's container was never started")
  $.jobs[0].attempts[0].startedAt / .stoppedAt: missing (expected present)
  $.jobs[0].attempts[0].container.exitCode: missing (expected 0, got None)
  $.jobs[0].attempts[0].container.networkInterfaces: missing (expected [{attachmentId, privateIpv4Address}], got None)
  $.jobs[0].startedAt / $.jobs[0].stoppedAt: missing
  $.jobs[0].dependsOn: missing (expected [])
  $.jobs[0].parameters: missing (expected {})
  $.jobs[0].eksAttempts: missing (expected [])
  $.jobs[0].container.volumes / .environment / .mountPoints / .ulimits / .secrets: missing (expected [])
  $.jobs[0].container.exitCode: missing (expected 0)
  $.jobs[0].container.networkInterfaces: missing
  $.jobs[0].container.fargatePlatformConfiguration: missing (expected {'platformVersion': 'LATEST'})

Fixed vs variable text: `Essential container in task exited` is wholly fixed AWS text for this status. The recorded taskArn, attachmentId, privateIpv4Address, log stream name, job id and the startedAt/stoppedAt epoch values are per-run and are NOT requirements — only their presence and the status/statusReason are.

Model citations: Smithy DescribeJobs (POST /v1/describejobs, 200, output DescribeJobsResponse.jobs: JobDetailList, errors ClientException/ServerException); SubmitJob (POST /v1/submitjob, 200, SubmitJobResponse.{jobArn, jobName, jobId}); JobStatus enum [SUBMITTED, PENDING, RUNNABLE, STARTING, RUNNING, SUCCEEDED, FAILED]; JobDetail.{status, statusReason, startedAt, stoppedAt, attempts: AttemptDetails, dependsOn, parameters, container, eksAttempts}; AttemptDetail.{container: AttemptContainerDetail, startedAt, stoppedAt, statusReason}; AttemptContainerDetail.{taskArn, exitCode, logStreamName, networkInterfaces}.

## Steps to Reproduce
Replay fixture batch-fargate-job-runs-to-succeeded up to step task#17: register a FARGATE container job definition, create a FARGATE compute environment and a job queue attached to it, call SubmitJob, then poll DescribeJobs with the returned jobId until a terminal status. AWS reaches status SUCCEEDED with statusReason 'Essential container in task exited', one attempts[] entry (container.exitCode 0, taskArn, logStreamName, networkInterfaces) and the full JobDetail/ContainerDetail member set including the empty collections and fargatePlatformConfiguration {platformVersion: LATEST}. The emulator settles the job FAILED with statusReason "This emulator does not run containers, so the job's container was never started" and omits attempts' timestamps/exit code/network interfaces, the job's startedAt/stoppedAt, dependsOn, parameters, eksAttempts and the container collections and defaults.
