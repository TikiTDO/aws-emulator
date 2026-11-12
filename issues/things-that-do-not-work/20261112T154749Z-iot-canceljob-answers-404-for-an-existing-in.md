Issue-Source: trace-sweep-divergence
Issue-Trace: iot-job-continuous-lifecycle
Issue-Step: task#12
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-job-continuous-lifecycle
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot|CancelJob|2xx->4xx|$.description
Issue-Key: iot|CancelJob|2xx->4xx|$.jobArn
Issue-Key: iot|CancelJob|2xx->4xx|$.jobId
Issue-Key: iot|CancelJob|2xx->4xx|status
Issue-Commit: 2aaa5a298b102bf0987c01ba9c9df4294351381c

# iot CancelJob answers 404 for an existing in-progress job instead of 200 with the job's identifiers

> _Note: this issue was identified against the code base at commit `2aaa5a298b102bf0987c01ba9c9df4294351381c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelJob

## Description
Cause 1 of 1: CancelJob refuses a cancel on a job that exists and is cancellable, answering ResourceNotFoundException (404) with an empty success projection instead of AWS's 200 echo of the cancelled job's identifiers.

AWS: Smithy (com.amazonaws.iot#AWSIotService): CancelJob is PUT /jobs/{jobId}/cancel with success code 200. Its input is jobId (@required @httpLabel, JobId, @length 1..64, @pattern ^[a-zA-Z0-9_-]+$), reasonCode (@length 0..128, @pattern ^[\p{Upper}\p{Digit}_]+$), comment (@length 0..2028) and force (@httpQuery "force", boolean, @default false). Its output shape CancelJobResponse carries jobArn (JobArn), jobId (JobId) and description (JobDescription). Its modelled errors are InvalidRequestException (400), LimitExceededException (410), ResourceNotFoundException (404), ServiceUnavailableException (503) and ThrottlingException (400) — so 404 is reserved for a jobId that names no job in the account. Recorded behavior: for a job that was created and is IN_PROGRESS (a CONTINUOUS job targeting a thing group, whose description had just been updated), CancelJob answers 200 and echoes back the job's jobId, its jobArn (arn:aws:iot:<region>:<account>:job/<jobId>) and the job's CURRENT description — i.e. the description as last written by UpdateJob, not the one supplied at create.

Emulator: 404. None of jobArn, jobId or description is present in the body — the whole success projection is absent. The jobId in the request is the same one the fixture created earlier in the flow, so the emulator is treating a live job as non-existent.

Why: CancelJob is the only way a client stops a CONTINUOUS job from picking up newly-joining devices. A client that reads 404 concludes the job it created and just updated has vanished, so it will not retry the cancel (404 is not retryable) and will either abandon the job — leaving it running forever from the client's point of view — or crash on the absent jobArn/jobId/description it is contractually guaranteed. Because the answer is a not-found rather than a conflict or validation error, the client also cannot distinguish "this job is not cancellable in its current state" from "this job does not exist", which are the two cases it must branch on.

Fix: the CancelJob handler and its lookup against the job state — resolve jobId against the stored job the same way DescribeJob/UpdateJob do, transition it to CANCELED, and serialize the CancelJobResponse projection (jobArn, jobId, description read from the job's current state). The 404 branch must be reachable only for a jobId absent from the job store.

Traces: iot-job-continuous-lifecycle#task#12 (CancelJob; expected 200, got 404).

Note on related open issues that do NOT cover this cause: '20261107T022027Z-iot-jobs-and-commands-plane-8-causes-continuous.md' carries replay keys for CreateJob, DescribeJob, DeleteJob, DeleteCommandExecution and GetCommand in this same flow and names CancelJob nowhere — neither in its Issue-Key list nor in any of its eight causes — so no replay of that issue exercises this step. '20261106T201842Z-iot-jobs-plane-8-net-new-causes-createjob.md' is a behavioral finding and holds no fixture step. The replay reports no stub blocking this step, so CancelJob is implemented and answering wrongly rather than unimplemented. Hypothesis: the same create-time refusal that makes CreateJob reject a thing-group target leaves no job record for this id, in which case fixing that create path may also clear this step — but CancelJob's own lookup and its empty 404 projection are what this issue is grounded in, and they must be confirmed by re-replaying the step above.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout. Expected = recorded AWS response, Actual = emulator.

CancelJob, fixture iot-job-continuous-lifecycle, step task#12 (cluster of 1, verdict mismatch):
  - status: expected 200, got 404
  - $.jobId: expected present (the job id the flow created), got absent
  - $.jobArn: expected present (arn:aws:iot:<region>:<account>:job/<jobId>), got absent
  - $.description: expected the job's current description as last set by UpdateJob, got absent

The recording's concrete values were jobId 'sim2real-probe-iot-job-e411e088', jobArn 'arn:aws:iot:us-west-2:000000000000:job/sim2real-probe-iot-job-e411e088' and description 'updated probe job'. Those are this run's generated name, account and region and are NOT the requirement — the requirement is the 200 status and the presence of all three members, with jobId echoing the request's path label, jobArn built from the request's region/account and that jobId, and description read from the job's current stored state.

The emulator's 404 carries no body members at all, so there is no error code or message text to compare; the divergence is the status class itself (a modelled ResourceNotFoundException returned for a job that is present) plus the entirely absent success projection.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
3. Observe the divergence cluster for service iot, operation CancelJob, fixture iot-job-continuous-lifecycle, step task#12: expected status 200, actual 404.
4. Equivalently by hand against the emulator's iot endpoint: create a thing group; POST /jobs/<jobId> with targets=[that group's ARN] and targetSelection=CONTINUOUS; PATCH /jobs/<jobId> to set a new description; then PUT /jobs/<jobId>/cancel. AWS answers 200 with {jobId, jobArn, description} where description is the updated one; the emulator answers 404 with no body members.
5. Confirm a fix by re-replaying iot-job-continuous-lifecycle#task#12 and checking the cluster is gone.
