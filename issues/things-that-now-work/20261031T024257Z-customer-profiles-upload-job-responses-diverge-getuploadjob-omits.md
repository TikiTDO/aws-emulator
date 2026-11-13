Issue-Source: trace-sweep-divergence
Issue-Trace: customer-profiles-create-upload-job-lifecycle
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: customer-profiles-create-upload-job-lifecycle
Issue-Shape-Id: com.amazonaws.customerprofiles#CustomerProfiles_20200815
Issue-Key: customer-profiles|GetUploadJob|2xx->2xx|$.ResultsSummary
Issue-Key: customer-profiles|CreateUploadJob|4xx->4xx|$.Message
Issue-Commit: 0bb1f1b79094f028995043e29b30af1a1ea0de5c

# customer-profiles upload-job responses diverge: GetUploadJob omits ResultsSummary and CreateUploadJob mis-words domain-not-found

> _Note: this issue was identified against the code base at commit `0bb1f1b79094f028995043e29b30af1a1ea0de5c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetUploadJob

## Description
This bundle carries 2 net-new root causes, both in the customer-profiles upload-job surface (CreateUploadJob / GetUploadJob) exercised by the same lifecycle fixture. A fixer must close BOTH before submitting.

Cause 1 of 2: GetUploadJob omits the ResultsSummary structure from its response entirely.

AWS: Smithy GetUploadJob — http {GET /domains/{DomainName}/upload-jobs/{JobId}, code 200}, readonly; input DomainName (httpLabel, required, target name, @length 1..64, @pattern ^[a-zA-Z0-9_-]+$) and JobId (httpLabel, required, target uuid, @pattern ^[a-f0-9]{32}$); output shape GetUploadJobResponse with members JobId, DisplayName, Status (enum UploadJobStatus), StatusReason, CreatedAt, CompletedAt, Fields, UniqueKey, ResultsSummary (target structure ResultsSummary, jsonName "ResultsSummary"), DataExpiry. ResultsSummary is a structure with members UpdatedRecords, CreatedRecords and FailedRecords, each target optionalLong (doc: "The summary of results for an upload job, including the number of updated, created, and failed records"). AWS always serializes the ResultsSummary object on a GetUploadJob describe of an existing job; for a job that has not processed any records yet it is present as an object whose three counters are not yet populated (recorded response: ResultsSummary == {"CreatedRecords": null, "FailedRecords": null, "UpdatedRecords": null}). The presence of the member is the requirement; the individual counter values are job state, not fixed text.

Emulator: answers GetUploadJob with HTTP 200 and the rest of the envelope, but the ResultsSummary key is absent from the JSON body altogether (replay detail: "$.ResultsSummary: missing (expected {'CreatedRecords': None, 'FailedRecords': None, 'UpdatedRecords': None}, got None)"). Status is 200 both sides; the divergence is confined to the missing output member.

Why: a client that polls GetUploadJob to track ingestion progress reads ResultsSummary.CreatedRecords / UpdatedRecords / FailedRecords. Against the emulator the member is missing, so a typed SDK deserializes ResultsSummary as absent/None and any code that dereferences it to read a counter raises a null/attribute error rather than seeing not-yet-populated counters; progress and failed-record reporting cannot be implemented against the emulator at all, even though the job itself was created and described successfully.

Fix: the GetUploadJob response construction — the upload-job record in customer-profiles state must carry a results-summary sub-structure, and the GetUploadJob serializer must always emit the ResultsSummary object (with its three optionalLong counters unset until the job has processed records) rather than dropping the member when the counters are empty.

Traces: customer-profiles-create-upload-job-lifecycle#task#3.

Cause 2 of 2: CreateUploadJob's ResourceNotFoundException for a non-existent domain uses non-AWS message wording.

AWS: Smithy CreateUploadJob — http {POST /domains/{DomainName}/upload-jobs, code 200}; DomainName is an httpLabel; modeled errors AccessDeniedException(403), BadRequestException(400), InternalServerException(500), ResourceNotFoundException(404, "The requested resource does not exist, or access was denied", single member Message), ThrottlingException(429). Creating an upload job in a domain that does not exist is a 404 ResourceNotFoundException whose Message, per the recorded response, is "Domain with name <DomainName> could not be found". Fixed text: "Domain with name " and " could not be found"; the single variable part is the DomainName taken verbatim from the request URI label (in this recording the per-run name sim2real-probe-cp-missing-<suffix>, which is a request value and not part of the template). The exact wording is recorded behavior only — the model documents ResourceNotFoundException in prose and does not fix the Message text.

Emulator: returns the correct HTTP status 404 and the correct error shape, but its own sentence: $.Message == "The domain <DomainName> does not exist" (replay detail: expected 'Domain with name sim2real-probe-cp-missing-6287eb90 could not be found', got 'The domain sim2real-probe-cp-missing-6287eb90 does not exist'). The fixed text differs on both sides of the interpolated name.

Why: clients and conformance suites classify the missing-domain case by matching the error message (equality, prefix or regex on "Domain with name ... could not be found") because ResourceNotFoundException is reused for domains, object types and jobs alike and the message is the only discriminator. Against the emulator that match fails, so a missing domain is reported as an unclassified not-found and operator-facing logs show text AWS never emits.

Fix: the CreateUploadJob handler's domain-lookup/validation branch — the ResourceNotFoundException it raises when the DomainName label resolves to no domain must carry AWS's fixed wording with the request's DomainName interpolated; the status and error shape it already produces are correct. Note this is the same class of message-wording defect as open issue 20261030T060614Z-customer-profiles-batchgetprofile-emits-a-non-aws-errors.md (BatchGetProfile Errors[].Message), but a different operation and code path, so that issue does not cover it.

Traces: customer-profiles-create-upload-job-lifecycle#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, 55 steps, 29 passed).

Cause 1 — fixture customer-profiles-create-upload-job-lifecycle, step task#3, operation GetUploadJob. Verdict mismatch; expected status 200, actual status 200.
Expected (recorded AWS): $.ResultsSummary == {"CreatedRecords": null, "FailedRecords": null, "UpdatedRecords": null} — the member is present as an object with its three optionalLong counters not yet populated.
Actual (emulator): $.ResultsSummary missing from the body (got None).
Replay divergence key: service=customer-profiles|GetUploadJob|status=200->200|keys=$.ResultsSummary (cluster size 1).
Smithy: GetUploadJobResponse.ResultsSummary -> structure ResultsSummary{UpdatedRecords, CreatedRecords, FailedRecords : optionalLong}, jsonName "ResultsSummary"; http GET /domains/{DomainName}/upload-jobs/{JobId}, code 200.

Cause 2 — fixture customer-profiles-create-upload-job-lifecycle, step task#4, operation CreateUploadJob. Verdict mismatch; expected status 404, actual status 404 — divergence confined to $.Message.
Expected (recorded AWS): $.Message == "Domain with name sim2real-probe-cp-missing-6287eb90 could not be found"; fixed text is "Domain with name " + " could not be found", the variable part is the request's DomainName.
Actual (emulator): $.Message == "The domain sim2real-probe-cp-missing-6287eb90 does not exist".
Replay divergence key: service=customer-profiles|CreateUploadJob|status=404->404|keys=$.Message (cluster size 1).
Smithy: CreateUploadJob errors include ResourceNotFoundException (client, httpError 404, members {Message}); http POST /domains/{DomainName}/upload-jobs, code 200.

## Steps to Reproduce
1. Clone this checkout, boot the emulator and replay fixture customer-profiles-create-upload-job-lifecycle (e.g. via replay_wire_corpus()).
2. The fixture creates a customer-profiles domain, then calls CreateUploadJob on POST /domains/{DomainName}/upload-jobs with DisplayName, Fields and UniqueKey, receiving a 32-hex JobId.
3. Cause 1 (step task#3): call GetUploadJob on GET /domains/{DomainName}/upload-jobs/{JobId} for that freshly created job. Inspect the 200 body: AWS includes ResultsSummary as an object with CreatedRecords/FailedRecords/UpdatedRecords not yet populated; the emulator omits the ResultsSummary key entirely.
4. Cause 2 (step task#4): call CreateUploadJob with a DomainName that matches ^[a-zA-Z0-9_-]+$ but names no existing domain. Both sides answer HTTP 404 ResourceNotFoundException; read $.Message. AWS returns "Domain with name <DomainName> could not be found"; the emulator returns "The domain <DomainName> does not exist".
5. Equivalently, replay_wire_corpus() reports the clusters service=customer-profiles|GetUploadJob|status=200->200|keys=$.ResultsSummary and service=customer-profiles|CreateUploadJob|status=404->404|keys=$.Message as mismatches.
