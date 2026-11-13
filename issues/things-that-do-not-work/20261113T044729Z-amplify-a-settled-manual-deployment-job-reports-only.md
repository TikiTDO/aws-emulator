Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: amplify.manual-deployment-job-runs-two-hosting-steps-and-pads-the-job-arn
Issue-Shape-Id: com.amazonaws.amplify#Amplify
Issue-Key: amplify.manual-deployment-job-runs-two-hosting-steps-and-pads-the-job-arn|run#10|AssertionError|assert steps == [("DEPLOY", "SUCCEED"), ("VERIFY", "SUCCEED")], f"job steps {steps!r}"
Issue-Commit: 95a90eef1b842df7513213eb6e705ebd283a5cc0

# amplify: a settled manual deployment job reports only the DEPLOY step — the VERIFY step AWS appends is never emitted

> _Note: this issue was identified against the code base at commit `95a90eef1b842df7513213eb6e705ebd283a5cc0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetJob

## Description
One net-new root cause in the amplify service, grounded in the behavioral check listed below.

Cause 1 of 1: a manual deployment job (CreateDeployment -> PUT the zip to zipUploadUrl -> StartDeployment) that has reached the terminal status SUCCEED is projected by GetJob with exactly ONE execution step, named DEPLOY. AWS reports TWO hosting steps for this job kind: the deploy step followed by a VERIFY step, both SUCCEED.
  AWS: Smithy (services/service-com-amazonaws-amplify-amplify/smithy-model/com-amazonaws-amplify-amplify.json) models GetJob -> GET /apps/{appId}/branches/{branchName}/jobs/{jobId}, 200, output `job` @required -> Job{summary @required -> JobSummary, steps @required -> Steps (list of Step)}. Step requires stepName (@length 0..255), startTime, status (JobStatus enum) and endTime, and optionally carries logUrl, artifactsUrl, testArtifactsUrl, testConfigUrl, screenshots (map), statusReason and context. The model does not fix the step SEQUENCE, so the sequence is recorded behavior only: for a manual (zip-upload) deployment that settles SUCCEED, the recording shows the job's `steps` list holding more than one element — a deploy step with status SUCCEED, followed by a step whose `stepName` is the fixed literal 'VERIFY' with `status` 'SUCCEED' and its own startTime/endTime. The requirement is the presence and ORDER of those two steps and their terminal status; the timestamps, URLs and screenshot entries are per-run environment values and are never the requirement.
  Emulator: GetJob on the settled job reports `steps` of length 1: the check read [('DEPLOY', 'SUCCEED')] where AWS reports [('DEPLOY', 'SUCCEED'), ('VERIFY', 'SUCCEED')]. The job's other asserted properties up to that point matched (jobId '1' from CreateDeployment, summary.status 'SUCCEED', the ten-digit-padded jobArn ending '/branches/main/jobs/0000000001'), so the divergence is confined to the step list: the settle path collapses the whole job into a single step named by the job's conclusion and never appends the post-deploy VERIFY step.
  Why: a client reads `job.steps` to report deployment progress and to decide whether the deployment was verified. Against the emulator a console, CI gate or deployment-waiter that iterates the steps sees the deploy step succeed but finds no VERIFY step, so it concludes the post-deploy verification never ran (or, if it requires a VERIFY/SUCCEED pair before declaring the branch live, it fails or hangs on a deployment AWS reports as fully finished). Any step-count or last-step assertion also reads the wrong step as the job's final one.
  Fix: the job step projection in this crate's job state — the settle/JobConclusion path in services/service-com-amazonaws-amplify-amplify/src/state.rs that rebuilds `job.steps` on termination, together with the StartDeployment handler that seeds the step list (services/service-com-amazonaws-amplify-amplify/src/ops/start_deployment.rs). It must emit the full step sequence AWS reports for a settled manual deployment (deploy step, then a VERIFY step with stepName 'VERIFY', status SUCCEED and its own startTime/endTime) rather than a single conclusion-named step.
  Note on existing issues: the open issue 20261110T142528Z-amplify-3-net-new-causes-getjob-reports-a.md describes the same one-step projection, but it is grounded in the WIRE corpus and carries only wire replay keys (amplify|GetJob|2xx->2xx|$.job.steps[*] and the amplify-s3-prefix-deployment-job-succeeds#task#7 step); it holds no behavioral check id, and the replay report did not mark this check as owned by it, so this behavioral check id is filed here so it is re-replayed with the fix.
  Traces: behavioral check amplify.manual-deployment-job-runs-two-hosting-steps-and-pads-the-job-arn.

## Evidence
Check amplify.manual-deployment-job-runs-two-hosting-steps-and-pads-the-job-arn, verdict mismatch, GetJob answering HTTP 200:

  AssertionError: job steps [('DEPLOY', 'SUCCEED')]
  at: assert steps == [("DEPLOY", "SUCCEED"), ("VERIFY", "SUCCEED")], f"job steps {steps!r}"
  where steps = [(s["stepName"], s["status"]) for s in job["steps"]]

Expected (recorded AWS behavior): job.steps projects two hosting steps in order — the deploy step with status 'SUCCEED', then a step with stepName exactly 'VERIFY' and status 'SUCCEED'. 'VERIFY' and 'SUCCEED' are fixed text; each step's startTime/endTime (and any logUrl/artifactsUrl/screenshots values) are per-run values, not part of the requirement.
Actual: job.steps has length 1 — only ('DEPLOY', 'SUCCEED'); no VERIFY step exists.

The assertions preceding it passed, so they are not part of this divergence: CreateDeployment returned jobId '1', summary.status settled 'SUCCEED', and summary.jobArn ended with the ten-digit-padded '/branches/main/jobs/0000000001'. The check stopped at the step-list assertion, so the later activeJobId/totalNumberOfJobs/ListJobs/edge-GET assertions in the same check were never reached and are not claimed here.

Smithy cited: GetJob output Job{summary @required, steps @required -> list<Step>}; Step{stepName @required @length 0..255, startTime @required, status @required (JobStatus), endTime @required, logUrl?, artifactsUrl?, testArtifactsUrl?, testConfigUrl?, screenshots?, statusReason?, context?}. The model does not constrain which steps a deployment job runs — the two-step DEPLOY-then-VERIFY sequence is recorded behavior only.

## Steps to Reproduce
1. app = amplify.create_app(name=<unique>); appId = app['app']['appId'].
2. amplify.create_branch(appId=appId, branchName='main').
3. dep = amplify.create_deployment(appId=appId, branchName='main'); PUT a zip containing index.html to dep['zipUploadUrl'] (expect HTTP 200).
4. amplify.start_deployment(appId=appId, branchName='main', jobId=dep['jobId']).
5. Poll amplify.get_job(appId=appId, branchName='main', jobId=dep['jobId']) until job['summary']['status'] is terminal (it settles 'SUCCEED').
6. Read [(s['stepName'], s['status']) for s in job['steps']].
   Expected: [('DEPLOY', 'SUCCEED'), ('VERIFY', 'SUCCEED')] — two hosting steps, the VERIFY step last.
   Observed: [('DEPLOY', 'SUCCEED')] — a single step; the VERIFY step is absent.

Replay: re-run the behavioral check named in trace_ids.
