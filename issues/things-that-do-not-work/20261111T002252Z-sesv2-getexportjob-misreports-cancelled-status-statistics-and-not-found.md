Issue-Source: trace-sweep-divergence
Issue-Trace: sesv2-cancel-export-job-twice
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sesv2-cancel-export-job-twice sesv2-export-job-cancel-then-inspect sesv2-metrics-export-create-get-cancel sesv2-metrics-export-lifecycle sesv2-export-job-not-found-errors sesv2-message-insights-export-json sesv2-message-insights-export-lifecycle
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2|GetExportJob|2xx->2xx|$.JobStatus
Issue-Key: sesv2|GetExportJob|2xx->2xx|$.Statistics
Issue-Key: sesv2|GetExportJob|4xx->4xx|$.message
Issue-Key: sesv2|GetExportJob|2xx->2xx|$.Statistics.ExportedRecordsCount
Issue-Commit: f28dfafa5fc9e548e0b0df8415a880c6df425385

# GetExportJob misreports cancelled status, Statistics and not-found message

> _Note: this issue was identified against the code base at commit `f28dfafa5fc9e548e0b0df8415a880c6df425385`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetExportJob

## Description
Four net-new causes on the SESv2 export-job read path. All four are GetExportJob (GET /v2/email/export-jobs/{JobId}) returning a body or an error that does not match AWS: the job's lifecycle status is not advanced by a cancel, the Statistics structure is emitted under the wrong conditions in both directions, and the not-found refusal uses an emulator-authored sentence.

Cause 1 of 4: a cancelled export job is still reported as CREATED. AWS: CancelExportJob (PUT /v2/email/export-jobs/{JobId}/cancel, 200 with an empty body per Smithy CancelExportJob output {}) moves the job into the terminal CANCELLED state, and a subsequent GetExportJob reports JobStatus CANCELLED — Smithy: GetExportJobResponse.JobStatus targets the JobStatus enum whose values are [CREATED, PROCESSING, COMPLETED, FAILED, CANCELLED], so CANCELLED is the modelled terminal value a cancel produces. The same response also carries the Statistics structure (see Cause 2). Emulator: after the cancel the job is read back with JobStatus 'CREATED' and no Statistics member at all, i.e. CancelExportJob is accepted (200) but records no state change. Why: a client that cancels an export and then polls GetExportJob for a terminal status never observes one — it either loops until its own timeout or concludes the cancel silently failed and retries it, and a client that branches on CANCELLED to stop consuming the destination keeps waiting for an export that will never be produced. Fix: the export-job state in the service's job store, written by the CancelExportJob handler and read by GetExportJob — the cancel must transition the stored job's status to CANCELLED (and GetExportJob must serialize it). Traces: sesv2-cancel-export-job-twice#task#4; the same cluster also at sesv2-export-job-cancel-then-inspect#task#5, sesv2-metrics-export-create-get-cancel#teardown#6 and sesv2-metrics-export-lifecycle#teardown#5.

Cause 2 of 4: GetExportJob omits the Statistics structure entirely. AWS: GetExportJobResponse includes Statistics (Smithy: Statistics targets ExportStatistics {ProcessedRecordsCount, ExportedRecordsCount}), and the recording shows the structure PRESENT in the response for a job that has not yet produced counts, with its two integer members simply unset — the container is serialized, the counts are not. Emulator: the Statistics member is absent from the response body. Why: an SDK client that reads response.statistics() to decide whether an export has produced records gets a null container and must special-case it, and code that dereferences the structure before reading the counts breaks on a response AWS always shapes. Fix: the GetExportJob serializer (and the job record's statistics field, which should exist with unset counts from job creation rather than being absent). Traces: sesv2-export-job-cancel-then-inspect#task#3; the same cluster also at sesv2-metrics-export-create-get-cancel#task#3 and sesv2-metrics-export-lifecycle#task#3.

Cause 3 of 4: the not-found refusal for an unknown job id uses the wrong message text. AWS: GetExportJob for a job id that does not exist is refused 404 NotFoundException (Smithy: GetExportJob errors include NotFoundException with httpError 404, 'The resource you attempted to access doesn't exist.'), and the recorded message is 'Job ID: <JOBID> not found' — the fixed text is 'Job ID: <' ... '> not found' including the literal angle brackets, and the single variable part is the JobId taken verbatim from the request URI label. Emulator: the status (404) and error type are right, but the message is 'Export job JOBID does not exist.' — emulator-authored wording, no angle brackets, different sentence. Why: a client that matches or parses AWS's message to distinguish a missing job from another 404 condition does not recognise the refusal, and operators grepping logs for AWS's phrasing see nothing. Fix: the GetExportJob handler's not-found error message. Traces: sesv2-export-job-not-found-errors#task#1.

Cause 4 of 4: GetExportJob invents a zero ExportedRecordsCount for a message-insights export. AWS: ExportStatistics.ExportedRecordsCount is documented 'The number of records that were exported to the final export file. This value might not be available for all export source types' — for this export source the member is NOT emitted (the Statistics structure is present, ExportedRecordsCount is unset), so absence is the modelled way of saying 'not available for this source type'. Emulator: emits ExportedRecordsCount 0. Why: 0 is a meaningful value, not a stand-in for unknown — a client reporting export progress or asserting that an export produced rows reads a definite 'zero records exported' where AWS said 'this count is unavailable', and will report a successful export as empty. Fix: the GetExportJob serializer / the export-job statistics model — leave the count unset instead of defaulting it to 0 when no count is available for the export source type. Traces: sesv2-message-insights-export-json#task#3; the same cluster also at sesv2-message-insights-export-lifecycle#task#3.

All four causes are net-new: the open issue 20261029T171854Z-sesv2-metrics-export-job-insights-and-certificate-paths-skip.md covers the CreateExportJob request-validation gaps (inverted date range, custom S3 URL) and the VDM/insights/certificate paths, and carries no replay key for any GetExportJob response-shape or status-transition step, so it does not cover these.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, 269 steps, 194 passed).

Cause 1 (sesv2-cancel-export-job-twice#task#4, GetExportJob, 200/200): $.JobStatus expected 'CANCELLED', got 'CREATED'; $.Statistics expected present ({'ExportedRecordsCount': <unset>, 'ProcessedRecordsCount': <unset>}), got absent. Cluster of 4: also sesv2-export-job-cancel-then-inspect#task#5, sesv2-metrics-export-create-get-cancel#teardown#6, sesv2-metrics-export-lifecycle#teardown#5.

Cause 2 (sesv2-export-job-cancel-then-inspect#task#3, GetExportJob, 200/200): $.Statistics expected present with both counts unset, got absent. Cluster of 3: also sesv2-metrics-export-create-get-cancel#task#3, sesv2-metrics-export-lifecycle#task#3.

Cause 3 (sesv2-export-job-not-found-errors#task#1, GetExportJob, 404/404): $.message expected 'Job ID: <JOBID> not found' (fixed text 'Job ID: <' / '> not found', variable part = the JobId from the request), got 'Export job JOBID does not exist.' Status and x-amzn-errortype (NotFoundException) match.

Cause 4 (sesv2-message-insights-export-json#task#3, GetExportJob, 200/200): $.Statistics.ExportedRecordsCount expected unset, got 0. Cluster of 2: also sesv2-message-insights-export-lifecycle#task#3.

Smithy (com.amazonaws.sesv2#SimpleEmailService_v2): GetExportJob GET /v2/email/export-jobs/{JobId} -> GetExportJobResponse{JobId, ExportSourceType, JobStatus, ExportDestination, ExportDataSource, CreatedTimestamp, CompletedTimestamp, FailureInfo, Statistics}; errors BadRequestException(400), NotFoundException(404), TooManyRequestsException(429). JobStatus enum values [CREATED, PROCESSING, COMPLETED, FAILED, CANCELLED]. ExportStatistics{ProcessedRecordsCount:int, ExportedRecordsCount:int} ('This value might not be available for all export source types'). CancelExportJob PUT /v2/email/export-jobs/{JobId}/cancel -> empty output.

## Steps to Reproduce
1. Replay fixtures sesv2-cancel-export-job-twice (task#4), sesv2-export-job-cancel-then-inspect (task#3, task#5), sesv2-export-job-not-found-errors (task#1) and sesv2-message-insights-export-json (task#3) from trace set simparity-wire-aws-v1.
2. Cause 1: CreateExportJob, then CancelExportJob on the returned JobId, then GetExportJob — compare $.JobStatus against CANCELLED (and repeat the cancel; the job must stay CANCELLED).
3. Cause 2: CreateExportJob, then GetExportJob immediately — check that the response body contains a Statistics object even though neither count is available.
4. Cause 3: GetExportJob with a JobId that was never created — compare the 404 body's $.message against AWS's 'Job ID: <id> not found' wording (angle brackets included).
5. Cause 4: CreateExportJob with a MESSAGE_INSIGHTS data source, then GetExportJob — check that Statistics.ExportedRecordsCount is absent rather than 0.
