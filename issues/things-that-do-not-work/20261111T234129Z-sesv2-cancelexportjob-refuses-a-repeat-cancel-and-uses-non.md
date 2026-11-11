Issue-Source: trace-sweep-divergence
Issue-Trace: sesv2-cancel-export-job-twice
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sesv2-cancel-export-job-twice sesv2-export-job-cancel-then-inspect sesv2-export-job-not-found-errors
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2|CancelExportJob|2xx->4xx|$
Issue-Key: sesv2|CancelExportJob|2xx->4xx|status
Issue-Key: sesv2|CancelExportJob|4xx->4xx|$.message
Issue-Commit: 2529620f784cf3cdb0d117874d357cc4ee02b901

# CancelExportJob refuses a repeat cancel and uses non-AWS not-found wording

> _Note: this issue was identified against the code base at commit `2529620f784cf3cdb0d117874d357cc4ee02b901`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelExportJob

## Description
Two net-new causes on the SESv2 CancelExportJob path (PUT /v2/email/export-jobs/{JobId}/cancel). Both are CancelExportJob answering with a refusal AWS does not produce, or with AWS's refusal worded by the emulator: a second cancel of an already-cancelled job is rejected instead of accepted, and the unknown-job-id refusal carries emulator-authored prose.

Cause 1 of 2: a repeated cancel of the same export job is refused 400 instead of accepted. AWS: CancelExportJob is idempotent for a job already in the CANCELLED state — it answers 200 with an empty body (Smithy: CancelExportJob output shape CancelExportJobResponse is empty, http {method: PUT, uri: /v2/email/export-jobs/{JobId}/cancel, code: 200}; the operation's modelled errors are BadRequestException(400), NotFoundException(404), TooManyRequestsException(429)). The recording confirms this: the second cancel of the same JobId returns 200 with body {}. Recorded behavior only for the idempotency rule itself — the model does not state which job states are cancellable, and the recording shows an already-CANCELLED job being cancelled again successfully. Emulator: returns 400 with {"__type": "BadRequestException", "message": "Cannot cancel an export job that has a <CANCELLED> status."} — an error class AWS does not raise for this input at all. Why: a client that cancels on a retry, on a teardown path, or from two concurrent workers sees a hard client error for an operation that already succeeded; the second cancel is exactly the shape a retrying SDK produces, so the retry turns a successful cancel into a failed call and the caller surfaces a spurious BadRequestException (or aborts a teardown). Fix: the CancelExportJob handler's state-transition guard — a job already in CANCELLED must be accepted and answered 200 with an empty body rather than rejected. Traces: sesv2-cancel-export-job-twice#task#5; the same cluster also at sesv2-export-job-cancel-then-inspect#task#6.

Cause 2 of 2: the not-found refusal for an unknown job id uses the wrong message text. AWS: CancelExportJob for a JobId that does not exist is refused 404 NotFoundException (Smithy: CancelExportJob errors include NotFoundException with httpError 404), and the recorded message is 'Job ID: <JOBID> not found' — the fixed text is 'Job ID: <' followed by '> not found', including the literal angle brackets, and the single variable part is the JobId taken verbatim from the request's {JobId} URI label. Emulator: the status (404) and error type match, but the message is 'Export job JOBID does not exist.' — emulator-authored wording, no angle brackets, different sentence. Why: an error message is part of the contract; a client or operator that matches AWS's phrasing to tell a missing export job from another 404 condition does not recognise the refusal, and log/alert rules keyed on AWS's sentence never fire. Fix: the CancelExportJob handler's not-found error message. Traces: sesv2-export-job-not-found-errors#task#2.

Both causes are net-new. The open issue 20261111T002252Z-sesv2-getexportjob-misreports-cancelled-status-statistics-and-not-found.md covers the GetExportJob read path (cancelled JobStatus not persisted, Statistics shaping, GetExportJob's not-found wording) and carries replay keys only for GetExportJob steps — it holds no key for either CancelExportJob step here, so neither cause is exercised by it. 20261029T171854Z-sesv2-metrics-export-job-insights-and-certificate-paths-skip.md covers CreateExportJob request validation and the VDM/insights/certificate paths, not CancelExportJob.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, 269 steps, 200 passed, 33 divergence clusters).

Cause 1 (sesv2-cancel-export-job-twice#task#5, CancelExportJob, expected 200, actual 400): $ value_mismatch — expected body {}, got {"__type": "BadRequestException", "message": "Cannot cancel an export job that has a <CANCELLED> status."}. Cluster of 2: also sesv2-export-job-cancel-then-inspect#task#6.

Cause 2 (sesv2-export-job-not-found-errors#task#2, CancelExportJob, 404/404): $.message value_mismatch — expected 'Job ID: <JOBID> not found' (fixed text 'Job ID: <' / '> not found'; the only variable part is the JobId from the request URI label), got 'Export job JOBID does not exist.' Status 404 and x-amzn-errortype NotFoundException match.

Smithy (services/service-com-amazonaws-sesv2-simpleemailservice-v2/smithy-model/com-amazonaws-sesv2-simpleemailservice-v2.json): CancelExportJob input CancelExportJobRequest{JobId @httpLabel @required @length min 1}; output CancelExportJobResponse {} (empty); http {method: PUT, uri: /v2/email/export-jobs/{JobId}/cancel, code: 200}; errors BadRequestException(400), NotFoundException(404), TooManyRequestsException(429).

## Steps to Reproduce
1. Replay fixtures sesv2-cancel-export-job-twice (task#5), sesv2-export-job-cancel-then-inspect (task#6) and sesv2-export-job-not-found-errors (task#2) from trace set simparity-wire-aws-v1.
2. Cause 1: CreateExportJob, then call CancelExportJob on the returned JobId TWICE. The second call must answer 200 with an empty JSON body, not 400 BadRequestException.
3. Cause 2: call CancelExportJob with a JobId that was never created and compare the 404 body's $.message against AWS's 'Job ID: <id> not found' wording (literal angle brackets around the requested id).
