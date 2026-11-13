Issue-Source: trace-sweep-divergence
Issue-Trace: amplify-afs-traj-manual-deploy-job-sequencing
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: amplify-afs-traj-manual-deploy-job-sequencing
Issue-Shape-Id: com.amazonaws.amplify#Amplify
Issue-Key: amplify|StopJob|2xx->2xx|$.jobSummary.commitId
Issue-Key: amplify|StopJob|2xx->2xx|$.jobSummary.commitMessage
Issue-Key: amplify|StopJob|2xx->2xx|$.jobSummary.commitTime
Issue-Key: amplify|StopJob|2xx->2xx|$.jobSummary.jobArn
Issue-Key: amplify|StopJob|2xx->2xx|$.jobSummary.jobType
Issue-Key: amplify|StopJob|2xx->2xx|$.jobSummary.startTime
Issue-Key: amplify|StopJob|2xx->2xx|$.jobSummary.status
Issue-Key: amplify|GetJob|2xx->2xx|$.job.summary.commitId
Issue-Key: amplify|GetJob|2xx->2xx|$.job.summary.commitMessage
Issue-Key: amplify|GetJob|2xx->2xx|$.job.summary.commitTime
Issue-Key: amplify|GetJob|2xx->2xx|$.job.summary.jobArn
Issue-Key: amplify|GetJob|2xx->2xx|$.job.summary.jobType
Issue-Key: amplify|GetJob|2xx->2xx|$.job.summary.startTime
Issue-Key: amplify|GetJob|2xx->2xx|$.job.summary.status
Issue-Key: amplify|CreateDeployment|4xx->4xx|$.message
Issue-Key: amplify|ListJobs|2xx->2xx|$.jobSummaries[*].commitId
Issue-Key: amplify|ListJobs|2xx->2xx|$.jobSummaries[*].commitMessage
Issue-Key: amplify|ListJobs|2xx->2xx|$.jobSummaries[*].commitTime
Issue-Key: amplify|ListJobs|2xx->2xx|$.jobSummaries[*].jobArn
Issue-Key: amplify|ListJobs|2xx->2xx|$.jobSummaries[*].jobId
Issue-Key: amplify|ListJobs|2xx->2xx|$.jobSummaries[*].jobType
Issue-Key: amplify|ListJobs|2xx->2xx|$.jobSummaries[*].startTime
Issue-Key: amplify|ListJobs|2xx->2xx|$.jobSummaries[*].status
Issue-Key: amplify|CreateDeployment|2xx->4xx|$.jobId
Issue-Key: amplify|CreateDeployment|2xx->4xx|$.zipUploadUrl
Issue-Key: amplify|CreateDeployment|2xx->4xx|status
Issue-Related: 20261029T215038Z-amplify-job-and-deployment-starts-that-aws-refuses.md
Issue-Commit: 89d9000d48177d926e2929575adaacae7fe257c5

# amplify job lifecycle and projection: reserved jobs report CREATED, StopJob never settles to CANCELLED, and a refused CreateDeployment reserves no job

> _Note: this issue was identified against the code base at commit `89d9000d48177d926e2929575adaacae7fe257c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopJob

## Description
Six net-new root causes in how the amplify emulator runs and projects a BRANCH JOB, each grounded in a replayed wire fixture step of the manual-deployment job-sequencing trajectory. They share one fault: the emulator's job record uses its own lifecycle (a `CREATED` reserved job, an asynchronous `CANCELLING` stop settled by a 500 ms clock) and its own numbering rule (only a SUCCESSFUL CreateDeployment spends a job id), where AWS reports a reserved job as `PENDING`, answers StopJob with the terminal `CANCELLED`, and spends a job number on every CreateDeployment call including the one it refuses. They are independent defects and must EACH be closed before this issue is submitted.

Related open issues, named rather than relied on: 20261102T092113Z-amplify-branch-and-job-projection-wrong-stage-default.md owns the replay keys for the EARLIER shape of the GetJob/StopJob steps (when they answered 404) and for amplify-afs-traj-s3-source-deploy-mistakes#task#12; those steps now answer 200 with wrong CONTENT, which is a different divergence and is filed here with its own replay keys. 20261029T215038Z-amplify-job-and-deployment-starts-that-aws-refuses.md covers StartDeployment/StartJob refusals, not any cause below.

Cause 1 of 6: the job a CreateDeployment reserved is reported with status CREATED, an unpadded job ARN, and commit/time/type members AWS does not report at all.
  AWS: Smithy models GetJob -> GET /apps/{appId}/branches/{branchName}/jobs/{jobId}, output `job` @required (Job{summary: JobSummary, steps}); JobSummary$status targets enum JobStatus (CREATED | PENDING | PROVISIONING | RUNNING | FAILED | SUCCEED | CANCELLING | CANCELLED) and commitId/commitMessage/commitTime/startTime/jobType are @required. Recorded behavior overrides that @required for a reserved manual deployment: the job reads back with status 'PENDING', an empty `steps` list, `jobId` the bare decimal number, `jobArn` ending in that number ZERO-PADDED TO TEN DIGITS (arn:aws:amplify:<region>:<account>:apps/<appId>/branches/<branch>/jobs/0000000001), and commitId, commitMessage, commitTime, startTime, jobType, sourceUrl, sourceUrlType and endTime ABSENT.
  Emulator: status 'CREATED' (services/service-com-amazonaws-amplify-amplify/src/state.rs JOB_STATUS_CREATED, chosen because "nothing has started it"), commitId '' and commitMessage '' as empty strings, commitTime and startTime set to the wall clock, jobType 'MANUAL', and jobArn '<branchArn>/jobs/<n>' with no padding (ops/create_deployment.rs `reserved_job_summary`).
  Why: a client polls a deployment by status and waits for the documented PENDING -> RUNNING -> SUCCEED progression; told 'CREATED' it either loops forever or treats the job as an unknown state. It also cannot tell "no commit information" from "an empty commit id", so commit-based correlation treats a manual deployment as a repository build with a blank commit, and an unpadded ARN is one AWS never emits, so IAM resource matching and log correlation built from it do not match.
  Fix: the CreateDeployment handler's reserved-job summary construction and the shared JobSummary/job-ARN projection in this crate's job state - report the reserved job as PENDING, omit the members AWS omits instead of defaulting them to '' or now(), and pad the ARN's job segment to ten digits while jobId stays bare.
  Traces: amplify-afs-traj-manual-deploy-job-sequencing#task#7.

Cause 2 of 6: StopJob answers the non-terminal CANCELLING where AWS answers the terminal CANCELLED, and repeats Cause 1's invented members and unpadded ARN.
  AWS: Smithy models StopJob -> DELETE /apps/{appId}/branches/{branchName}/jobs/{jobId}/stop, output `jobSummary` @required, errors BadRequestException (400) / NotFoundException (404). Recorded behavior: stopping the reserved deployment job answers HTTP 200 with jobSummary.status 'CANCELLED' - the TERMINAL status, in the call's own response - with the ten-digit-padded jobArn, the bare jobId, and the commit/time/type members absent.
  Emulator: answers status 'CANCELLING' and defers the terminal status to a clock JOB_SETTLE_MILLIS (500 ms) later (state.rs `BranchJob::cancel`, JOB_STATUS_CANCELLING; ops/stop_job.rs calls `job.cancel()` and returns the summary it left behind), plus commitId '', commitMessage '', commitTime/startTime wall-clock values, jobType 'MANUAL' and the unpadded jobArn.
  Why: StopJob is the documented escape from a blocked branch, and a client that stops a job and then checks terminality - or immediately creates the next deployment - is told the stop is still in flight. Because `is_terminal_job_status` excludes CANCELLING, the branch stays blocked for the whole span the clock has not elapsed, which is Cause 5.
  Fix: the StopJob handler and `BranchJob::cancel` - the cancellation of a job that has not started must be COMPLETE when the call answers (terminal CANCELLED, no settle instant owed), with the same JobSummary projection fix as Cause 1.
  Traces: amplify-afs-traj-manual-deploy-job-sequencing#task#8.

Cause 3 of 6: the CreateDeployment serialization refusal names the WRONG job id in its message, because a refused call spends no job number.
  AWS: Smithy models BadRequestException (HTTP 400, "A request contains unexpected data") for CreateDeployment. Recorded behavior: the message's fixed text is exactly "The last job(deployment) <jobId> was not finished, please wait until it finish or use StopJob operation to cancel it before creating a new deployment." and its ONE variable part is a job id read from the branch's job history. At the first refusal, with the branch's reserved job being number 1, AWS names 2; at the second refusal it names the next number again - i.e. the id named is the one THIS call reserved, which is now the branch's last job. Hypothesis for the mechanism (consistent across all four steps of this trajectory): AWS reserves the next job number and records a PENDING job for it on every CreateDeployment call, and only then applies the serialization rule, so the refusal names its own reservation.
  Emulator: names the id of the pre-existing unfinished job instead ('1'), because it evaluates `branch.last_unfinished_job()` and returns before `reserve_job_id()` runs, so a refused call consumes no number and stores no job (ops/create_deployment.rs, state.rs `unfinished_job_message`).
  Why: the message exists to tell the client which job to StopJob. Pointed at the wrong id, a client that automates the documented recovery stops a job it was not told about - or, in the other direction, calls StopJob for an id that does not exist on the emulator - and the branch is never unblocked.
  Fix: the CreateDeployment handler's serialization path and the branch job-number allocation in this crate's job state - reserve and record the job before the refusal is composed, and build the message from that reservation.
  Traces: amplify-afs-traj-manual-deploy-job-sequencing#task#4, and amplify-afs-traj-manual-deploy-job-sequencing#task#9 in the same cluster.

Cause 4 of 6: ListJobs reports one job where AWS reports the branch's whole numbered history.
  AWS: Smithy models ListJobs -> GET /apps/{appId}/branches/{branchName}/jobs, output `jobSummaries` @required, @paginated. Recorded behavior: after reserve -> stop -> two refused reserves, the branch's listing carries THREE summaries, newest first - job 3 PENDING, job 2 PENDING, job 1 CANCELLED - each with the bare jobId, the ten-digit-padded jobArn and the commit/time/type members absent. Every CreateDeployment call, including the ones AWS refuses, is present as a numbered job.
  Emulator: a single element - jobId '1', status 'CANCELLING', jobArn '.../jobs/1' unpadded, commitId '', commitMessage '', commitTime/startTime wall-clock values, jobType 'MANUAL' - and the elements for jobs 2 and 3 missing entirely. The ListJobs handler's paging and newest-first order are correct; the records simply were never created (Cause 3) and the one record carries the wrong status (Cause 2) and projection (Cause 1).
  Why: ListJobs is how a client reconciles a branch's deployment history, drives a teardown, or finds the job to stop. It reports one job where AWS has three, so every id the client holds from a CreateDeployment response is absent from the history and reconciliation acts on state that does not exist.
  Fix: no change in the ListJobs handler itself - it follows from Causes 1, 2 and 3 (job records for refused reservations, terminal CANCELLED, and the JobSummary/ARN projection), so re-replay this step after those.
  Traces: amplify-afs-traj-manual-deploy-job-sequencing#task#10.

Cause 5 of 6: a CreateDeployment that AWS accepts after the branch's last job was stopped is refused with 400, so a stopped branch can never be deployed again.
  AWS: Smithy models CreateDeployment -> POST /apps/{appId}/branches/{branchName}/deployments with output jobId / fileUploadUrls / zipUploadUrl and BadRequestException (400) among its errors. Recorded behavior: once the branch's LAST job has been stopped - i.e. reached CANCELLED - the next CreateDeployment answers HTTP 200 with the next job id and the upload destination for the zipped deployment; the serialization refusal applies only while that last job is unfinished.
  Emulator: answers HTTP 400 with the "The last job(deployment) ... was not finished ..." message and no jobId and no zipUploadUrl, because the job it stopped is still in the non-terminal CANCELLING status at that instant (Cause 2) and `is_terminal_job_status` therefore keeps the branch blocked.
  Why: StopJob is AWS's own documented way out of the "last job not finished" state, and here it does not unblock the branch for the client's very next call, so the branch is effectively permanently un-deployable through the emulator and the whole documented manual-deployment loop stops after the first cancellation.
  Fix: the StopJob terminality of Cause 2, plus the CreateDeployment serialization gate in ops/create_deployment.rs (`branch.last_unfinished_job`) - a stop that has been answered must leave the branch deployable.
  Traces: amplify-afs-traj-manual-deploy-job-sequencing#task#12.

Cause 6 of 6: the same ListJobs divergence one deployment round later - four jobs expected, one served.
  AWS: same ListJobs contract as Cause 4. Recorded behavior after the second round: FOUR summaries, newest first - job 4 PENDING, job 3 CANCELLED, job 2 PENDING, job 1 CANCELLED - with the same bare jobId / padded jobArn / absent commit-time-type projection.
  Emulator: the one stale element again (jobId '1', status 'CANCELLING', unpadded ARN, invented members); jobs 2, 3 and 4 are absent because the refused reservations stored nothing (Cause 3) and the accepted one was refused (Cause 5).
  Why: a client that lists a branch's jobs after two deployment rounds sees neither the deployment it just created nor the ones it cancelled, so deployment history and status dashboards are empty of everything but the first job.
  Fix: follows from Causes 1, 2, 3 and 5; this step is the end-to-end check that the branch's numbered history and the lifecycle statuses line up, so it must be re-replayed after all of them.
  Traces: amplify-afs-traj-manual-deploy-job-sequencing#task#13.

## Evidence
All steps are from fixture amplify-afs-traj-manual-deploy-job-sequencing (trace set simparity-wire-aws-v1). Region, account, appId, job ARNs' app/branch segments, timestamps and ports are per-run values; the requirements are the statuses, the absent-vs-present members, the ten-digit ARN padding, the id named in the message and the HTTP statuses.

Cause 1 (task#7, GetJob, HTTP 200 both sides): $.job.summary.status expected 'PENDING', actual 'CREATED'. $.job.summary.jobArn expected '.../branches/main/jobs/0000000001', actual '.../branches/main/jobs/1'. $.job.summary.commitId expected ABSENT, actual ''. commitMessage expected ABSENT, actual ''. commitTime expected ABSENT, actual a wall-clock timestamp. startTime expected ABSENT, actual a wall-clock timestamp. jobType expected ABSENT, actual 'MANUAL'.

Cause 2 (task#8, StopJob, HTTP 200 both sides): $.jobSummary.status expected 'CANCELLED', actual 'CANCELLING'. $.jobSummary.jobArn expected '.../jobs/0000000001', actual '.../jobs/1'. commitId expected ABSENT, actual ''. commitMessage expected ABSENT, actual ''. commitTime/startTime expected ABSENT, actual wall-clock timestamps. jobType expected ABSENT, actual 'MANUAL'.

Cause 3 (task#4, CreateDeployment, HTTP 400 both sides; task#9 in the same cluster): $.message expected "The last job(deployment) 2 was not finished, please wait until it finish or use StopJob operation to cancel it before creating a new deployment.", actual "The last job(deployment) 1 was not finished, please wait until it finish or use StopJob operation to cancel it before creating a new deployment." The sentence is fixed text; the only variable part is the job id, and the emulator fills it with the pre-existing unfinished job rather than the one this call reserves.

Cause 4 (task#10, ListJobs, HTTP 200 both sides): expected three elements newest-first - [0] jobId '3' status 'PENDING' jobArn '.../jobs/0000000003', [1] jobId '2' status 'PENDING' jobArn '.../jobs/0000000002', [2] jobId '1' status 'CANCELLED' jobArn '.../jobs/0000000001', all with commitId/commitMessage/commitTime/startTime/jobType/sourceUrl/sourceUrlType/endTime absent. Actual: ONE element - jobId '1', status 'CANCELLING', jobArn '.../jobs/1', commitId '', commitMessage '', commitTime and startTime set, jobType 'MANUAL'; $.jobSummaries[1] and $.jobSummaries[2] missing.

Cause 5 (task#12, CreateDeployment): expected HTTP 200 with $.jobId '4' and a $.zipUploadUrl. Actual HTTP 400 (BadRequestException, the "last job(deployment) ... was not finished" message), with no $.jobId and no $.zipUploadUrl.

Cause 6 (task#13, ListJobs, HTTP 200 both sides): expected four elements newest-first - jobId '4' PENDING, '3' CANCELLED, '2' PENDING, '1' CANCELLED, each with the ten-digit-padded jobArn and the commit/time/type members absent. Actual: ONE element - jobId '1', status 'CANCELLING', jobArn '.../jobs/1', commitId '', commitMessage '', commitTime/startTime set, jobType 'MANUAL'; $.jobSummaries[1], [2] and [3] missing.

## Steps to Reproduce
Setup (all causes): app = amplify.create_app(name=<unique>)['app']; amplify.create_branch(appId=app['appId'], branchName='main').

Cause 1:
1. d1 = amplify.create_deployment(appId, 'main', fileMap={'index.html': <md5>}).
2. amplify.get_job(appId, 'main', d1['jobId']) -> job.summary.status must be 'PENDING' (observed 'CREATED'), summary.jobArn must end '/jobs/0000000001' while jobId is '1' (observed '/jobs/1'), and commitId/commitMessage/commitTime/startTime/jobType must be ABSENT (observed '', '', now(), now(), 'MANUAL').

Cause 2:
3. amplify.stop_job(appId, 'main', d1['jobId']) -> jobSummary.status must be 'CANCELLED' in THIS response (observed 'CANCELLING'), with the padded jobArn and the absent members as above.

Cause 3:
4. With the branch's last job unfinished (repeat step 1, then call create_deployment again immediately): amplify.create_deployment(appId, 'main', fileMap={...}) -> 400 BadRequestException whose message must name the job id this call reserved, i.e. one greater than the outstanding job ('The last job(deployment) 2 was not finished, ...'); observed the id of the outstanding job ('... 1 was not finished, ...').

Causes 4 and 6:
5. amplify.list_jobs(appId, 'main') after the reserve -> stop -> two-refused-reserves sequence -> jobSummaries must carry one element per CreateDeployment call made, newest first (3 PENDING, 2 PENDING, 1 CANCELLED); observed a single element for job 1 with status 'CANCELLING'. Repeat after the next round and expect four elements (4 PENDING, 3 CANCELLED, 2 PENDING, 1 CANCELLED); observed the same single stale element.

Cause 5:
6. Immediately after a successful stop_job of the branch's last job, amplify.create_deployment(appId, 'main') -> must answer 200 with the next jobId and a zipUploadUrl; observed 400 with the 'last job(deployment) ... was not finished' message, because the stopped job is still 'CANCELLING' for ~500 ms.
