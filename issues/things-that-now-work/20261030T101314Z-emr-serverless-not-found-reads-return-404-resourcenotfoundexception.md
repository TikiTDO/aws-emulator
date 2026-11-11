Issue-Source: trace-sweep-divergence
Issue-Trace: emr-serverless-error-paths
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: emr-serverless-error-paths emr-serverless-start-stop-dashboard-endpoint-attempts-not-found
Issue-Shape-Id: com.amazonaws.emrserverless#AwsToledoWebService
Issue-Key: emr-serverless|GetDashboardForJobRun|4xx->4xx|$.message
Issue-Key: emr-serverless|GetDashboardForJobRun|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: emr-serverless|GetDashboardForJobRun|4xx->4xx|status
Issue-Key: emr-serverless|GetSessionEndpoint|4xx->4xx|$.message
Issue-Key: emr-serverless|GetSessionEndpoint|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: emr-serverless|GetSessionEndpoint|4xx->4xx|status
Issue-Commit: e63ec425c23513ba26a0bb6d325f047894e211cc

# EMR Serverless not-found reads return 404 ResourceNotFoundException instead of AWS 400 ValidationException

> _Note: this issue was identified against the code base at commit `e63ec425c23513ba26a0bb6d325f047894e211cc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDashboardForJobRun

## Description
The emulator maps every "referenced child resource is absent" condition on the EMR Serverless dashboard/endpoint read paths onto ResourceNotFoundException/HTTP 404, while AWS rejects these requests as input validation: HTTP 400 with x-amzn-errortype ValidationException and a message that names the offending identifier. Both operations modelled for this behavior diverge the same way, so the defect is in how the handlers classify a missing jobRunId/sessionId rather than in either operation alone. 2 causes.

Cause 1 of 2: GetDashboardForJobRun on a job-run id that does not exist under an existing application.
AWS: the operation models both ValidationException (@httpError 400) and ResourceNotFoundException (@httpError 404) (Smithy: GetDashboardForJobRun errors = InternalServerException/500, ResourceNotFoundException/404, ValidationException/400; both carry a single required `message` member of String1024). The recording shows AWS choosing the validation class for an unknown job-run id: HTTP 400, error shape ValidationException, body {"message": "Job Run with given ID doesn't exist."}. In that message every word is fixed text for this error class, including the terminating period; it embeds no value from the request.
Emulator: HTTP 404 with x-amzn-errortype: ResourceNotFoundException and message "Job Run with given ID doesn't exist" — the right sentence with the trailing period dropped, under the wrong status and wrong error shape.
Why: an SDK caller gets the wrong modelled exception type. Code that catches ValidationException (and treats a bad job-run id as a caller error) falls through, while code that catches ResourceNotFoundException and retries/creates the resource is entered on a request AWS considers malformed. Retry classifiers and error-text assertions both diverge.
Fix: the GetDashboardForJobRun handler's job-run lookup/validation path in the emr-serverless service crate — classify an unresolvable jobRunId as ValidationException (400) and emit the message with its terminating period.
Traces: emr-serverless-error-paths#task#6.

Cause 2 of 2: GetSessionEndpoint when the referenced session cannot be resolved.
AWS: same error set (Smithy: GetSessionEndpoint errors = InternalServerException/500, ResourceNotFoundException/404, ValidationException/400; input applicationId and sessionId are both @required @httpLabel, pattern ^[0-9a-z]+$). The recording shows HTTP 400 with body {"message": "SessionId is invalid."} — fixed text for the class, naming the input member (sessionId) rather than echoing its value, so nothing in the message is a per-run value.
Emulator: HTTP 404, x-amzn-errortype: ResourceNotFoundException, message "Application does not exist" — it attributes the failure to the parent application and uses its own generic not-found sentence instead of AWS's session-scoped validation message.
Why: the client is told the wrong resource is missing (application rather than session) in the wrong error class, so a caller that branches on "application gone" vs "session gone" takes the wrong branch, and the modelled exception it catches is the wrong one.
Fix: the GetSessionEndpoint handler's identifier resolution/validation in the emr-serverless service crate — return ValidationException (400) with the session-scoped message when the session cannot be resolved.
Traces: emr-serverless-start-stop-dashboard-endpoint-attempts-not-found#task#4.

No open emr-serverless issue covers error classification on these read paths; the existing open issues concern job-run execution/lifecycle and log publication.

## Evidence
Cause 1 — fixture emr-serverless-error-paths, step task#6, GetDashboardForJobRun.
  expected: status 400, $.message = "Job Run with given ID doesn't exist.", error shape ValidationException
  actual:   status 404, $.message = "Job Run with given ID doesn't exist" (no period), header x-amzn-errortype = ResourceNotFoundException
  replay detail: status: expected 400, got 404; $.message: value_mismatch; $headers.x-amzn-errortype: shape_mismatch

Cause 2 — fixture emr-serverless-start-stop-dashboard-endpoint-attempts-not-found, step task#4, GetSessionEndpoint.
  expected: status 400, $.message = "SessionId is invalid."
  actual:   status 404, $.message = "Application does not exist", header x-amzn-errortype = ResourceNotFoundException
  replay detail: status: expected 400, got 404; $.message: value_mismatch; $headers.x-amzn-errortype: shape_mismatch

Model grounding: Smithy ValidationException @error client @httpError 400, required member message: String1024; ResourceNotFoundException @error client @httpError 404. Both are bound to GetDashboardForJobRun and GetSessionEndpoint, so either status is modelled — the recorded AWS choice is the 400 validation class.

## Steps to Reproduce
1. Replay fixture emr-serverless-error-paths against the emulator and stop at step task#6: create an application, start a job run, then call GetDashboardForJobRun with a jobRunId that was never created (GET /applications/{applicationId}/jobruns/{unknownJobRunId}/dashboard). Observe 404/ResourceNotFoundException instead of 400/ValidationException, and the message missing its trailing period.
2. Replay fixture emr-serverless-start-stop-dashboard-endpoint-attempts-not-found and stop at step task#4: call GetSessionEndpoint (GET /applications/{applicationId}/sessions/{sessionId}/endpoint) for a session that does not exist. Observe 404 "Application does not exist" instead of 400 "SessionId is invalid.".
