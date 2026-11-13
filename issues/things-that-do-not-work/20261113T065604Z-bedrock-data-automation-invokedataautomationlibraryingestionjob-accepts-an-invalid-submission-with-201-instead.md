Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-data-automation-library-ingestion-job
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-data-automation-library-ingestion-job
Issue-Shape-Id: com.amazonaws.bedrockdataautomation#AmazonBedrockKeystoneBuildTimeService
Issue-Key: bedrock-data-automation|InvokeDataAutomationLibraryIngestionJob|4xx->2xx|$.message
Issue-Key: bedrock-data-automation|InvokeDataAutomationLibraryIngestionJob|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock-data-automation|InvokeDataAutomationLibraryIngestionJob|4xx->2xx|status
Issue-Commit: a30cb6a6b6905f387bbecd05254c1537fe1d9d44

# InvokeDataAutomationLibraryIngestionJob accepts an invalid submission with 201 instead of 400 ValidationException

> _Note: this issue was identified against the code base at commit `a30cb6a6b6905f387bbecd05254c1537fe1d9d44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeDataAutomationLibraryIngestionJob

## Description
One net-new root cause on the refusal path of InvokeDataAutomationLibraryIngestionJob (restJson1, PUT /data-automation-libraries/{libraryArn}/library-ingestion-jobs/, success status 201).

=== Cause 1 of 1: an ingestion submission AWS refuses as an invalid input parameter is ACCEPTED, and a job resource is minted for it ===

AWS: Smithy (services/service-com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice/smithy-model/com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice.json): InvokeDataAutomationLibraryIngestionJob takes libraryArn (@httpLabel, @required), inputConfiguration (@required -> InputConfiguration{s3Object, inlinePayload} — BOTH members optional in the model), entityType (@required, enum VOCABULARY|RECOGNIZED_PERSON), operationType (@required, enum UPSERT|DELETE), outputConfiguration (@required -> OutputConfiguration{s3Uri @required}); InlinePayload is a union{upsertEntitiesInfo -> UpsertEntitiesInfo(list @length 1..10), deleteEntitiesInfo -> DeleteEntitiesInfo}; UpsertEntityInfo is a union whose ONLY member is vocabulary -> VocabularyEntityInfo. Modeled errors include ValidationException (errorKind client, @httpError 400, members message + fieldList). The model expresses NO cross-member rule, so for the request this step sends the requirement is recorded behavior only: the real service refuses the submission with HTTP 400, error type ValidationException (header x-amzn-errortype, shaped but not asserted by the fixture), and the response body member `message` set to the fixed sentence "Invalid input parameter provided." — that sentence is entirely FIXED text: it has no variable part, nothing is interpolated from the request or from service state, and no field name, ARN, value or limit appears in it. Nothing in the recording is a per-run value, so the whole message is the requirement.

Hypothesis (NOT stated by the model or the recording, offered only to point the fix at the right rule): the refused request satisfies every constraint trait individually but breaks a cross-member rule the service enforces in the handler — the candidates the shape graph admits are (a) inputConfiguration naming BOTH s3Object and inlinePayload, (b) operationType disagreeing with the InlinePayload variant (UPSERT with deleteEntitiesInfo, or DELETE with upsertEntitiesInfo), and (c) entityType disagreeing with the UpsertEntityInfo variant (RECOGNIZED_PERSON with a `vocabulary` entry, the union's only variant). Whichever rule the recorded request breaks, AWS answers the SAME fixed sentence above, so the requirement is the refusal, not the hypothesis.

Emulator: the handler answers 201 with a freshly minted jobArn. The replayed step expected status 400 and the two assertions `$.message` = 'Invalid input parameter provided.' and `$headers.x-amzn-errortype` shaped; the emulator produced status 201 and NEITHER key is present at all (both reported `missing`, i.e. the body is a success payload with no `message` and the response carries no error type). The handler's semantic checks are limited to exactly two: the addressed library must exist in the store (else 404 ResourceNotFoundException "Library Arn not found.") and inputConfiguration must name at least one of s3Object / inlinePayload (else 400 ValidationException "inputConfiguration must specify either s3Object or inlinePayload."). No check relates inputConfiguration's two sources to each other, and none relates entityType or operationType to the InlinePayload / UpsertEntityInfo variant actually supplied, so a submission that breaks any of those rules falls straight through to the mint-and-store path.

Why: the call reports success for a request the real service rejects, and because the accept path mints and persists the job record BEFORE answering, the emulator is left holding a job the real service never created. A caller that drives this operation and branches on the refusal (retrying with a corrected payload, surfacing the ValidationException to a user, or asserting the invalid-input contract in a conformance test) instead takes the success branch, then observes that phantom job through GetDataAutomationLibraryIngestionJob and ListDataAutomationLibraryIngestionJobs and watches it transition IN_PROGRESS -> COMPLETED — so an invalid submission is reported as a completed ingestion, and the divergence propagates from one wrong status into the service's whole job state. Error-handling code paths for this operation therefore cannot be exercised against the emulator at all.

Fix: the operation handler's pre-mint validation in services/service-com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice/src/ops/invoke_data_automation_library_ingestion_job.rs — add the cross-member consistency refusal alongside the existing missing-source check, returning ValidationException (HTTP 400) whose message is the service's fixed sentence for an invalid input parameter, single-homed as a constant in crate::state::library_ingestion next to MISSING_INGESTION_INPUT_MESSAGE. It must refuse BEFORE any ARN is minted or any record is inserted into store.ingestion_jobs, so a refused submission leaves no job behind (the existing library-not-found and missing-source branches already have that ordering).

Related open issues, named so a fixer can single-home the change and NOT because either one covers this divergence (the replay marked this step neither blocked by a stub nor owned by an open issue): 20261103T163536Z-bedrock-data-automation-invokedataautomationlibraryingestionjob-stub.md asserts this operation is still an unimplemented scaffold stub, which the handler cited above shows is stale — the handler mints and stores jobs; and 20261108T152818Z-bedrock-data-automation-invokedataautomationlibraryingestionjob-required-member-validationexception-renders-value-null-at.md covers the WORDING of the pre-handler constraint-walk refusal for a @required/null violation ("1 validation error detected: Value at '<path>' ..."), a different message and a different validation phase from the handler-level invalid-parameter refusal that is missing entirely here.

Traces: bedrock-data-automation-library-ingestion-job#task#2 (InvokeDataAutomationLibraryIngestionJob).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, 70 steps, 57 passed, score 81.4%).

Divergence cluster (verdict: mismatch, service bedrock-data-automation, operation InvokeDataAutomationLibraryIngestionJob, affected_count 1, stub_path empty, owned_by empty):
  fixture_id: bedrock-data-automation-library-ingestion-job
  step_key:   task#2
  expected_status: 400, actual_status: 201
  detail:
    status: expected 400, got 201
    $.message: missing (expected 'Invalid input parameter provided.', got None)
    $headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)
  divergence_key: service=bedrock-data-automation|InvokeDataAutomationLibraryIngestionJob|status=400->201|keys=$.message,$headers.x-amzn-errortype,status

Expected (AWS, as recorded): HTTP 400, error type ValidationException, body message exactly "Invalid input parameter provided." — all fixed text, no part of it taken from the request or from state.
Actual (emulator): HTTP 201 with the success payload InvokeDataAutomationLibraryIngestionJobResponse{jobArn: "<minted ARN>"}; no `message` member and no x-amzn-errortype header. The minted ARN itself is a per-run generated value and is not part of the requirement.

Code evidence (services/service-com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice/src/ops/invoke_data_automation_library_ingestion_job.rs): handle() refuses only two things before minting — store.libraries.contains_key(&input.library_arn) is false -> ResourceNotFoundException with crate::state::LIBRARY_ARN_NOT_FOUND_MESSAGE (lines 188-197), and input_configuration.s3_object.is_none() && input_configuration.inline_payload.is_none() -> ValidationException with crate::state::library_ingestion::MISSING_INGESTION_INPUT_MESSAGE (lines 202-222). It then checks the idempotency token and the job quota and falls through to mint_ingestion_job_arn + store.ingestion_jobs.insert (lines 259-283). There is no branch anywhere that compares inputConfiguration's two sources to each other, or entityType / operationType to the InlinePayload or UpsertEntityInfo variant supplied, so no request can reach a 400 "Invalid input parameter provided.".

Model citations (same model file): InvokeDataAutomationLibraryIngestionJob — http PUT /data-automation-libraries/{libraryArn}/library-ingestion-jobs/, code 201, @idempotent; errors AccessDeniedException(403), ConflictException(409), InternalServerException(500), ResourceNotFoundException(404), ServiceQuotaExceededException(402), ThrottlingException(429), ValidationException(400, members message+fieldList). InputConfiguration{s3Object -> S3Object, inlinePayload -> InlinePayload} with neither member @required; InlinePayload = union{upsertEntitiesInfo -> UpsertEntitiesInfo(@length 1..10), deleteEntitiesInfo -> DeleteEntitiesInfo{entityIds @required}}; UpsertEntitiesInfo.member -> UpsertEntityInfo = union{vocabulary -> VocabularyEntityInfo}; EntityType = enum[VOCABULARY, RECOGNIZED_PERSON]; LibraryIngestionJobOperationType = enum[UPSERT, DELETE].

## Steps to Reproduce
1. Build and boot the emulator from this checkout (the wire replay reported emulator_up=true, /__admin/health 200).
2. Replay fixture bedrock-data-automation-library-ingestion-job and observe step task#2: it submits InvokeDataAutomationLibraryIngestionJob against the library the fixture's earlier step created, with an input the real service refuses as an invalid input parameter, and asserts HTTP 400 with body message "Invalid input parameter provided." and a shaped x-amzn-errortype of ValidationException.
3. Equivalently by hand: point a bedrock-data-automation client at the emulator, CreateDataAutomationLibrary to mint a libraryArn, create an S3 bucket for outputConfiguration.s3Uri, then call InvokeDataAutomationLibraryIngestionJob with every modeled constraint satisfied but a cross-member rule broken — e.g. operationType='UPSERT' with inputConfiguration={'inlinePayload': {'deleteEntitiesInfo': {'entityIds': ['e1']}}}, or entityType='RECOGNIZED_PERSON' with inputConfiguration={'inlinePayload': {'upsertEntitiesInfo': [{'vocabulary': {'entityId': 'e1', 'language': 'EN', 'phrases': [{'text': 'acme'}]}}]}}, or inputConfiguration naming BOTH 's3Object' and 'inlinePayload'.
4. Observe HTTP 201 and a response body carrying a minted jobArn, with no `message` member and no x-amzn-errortype header, where AWS answers 400 ValidationException "Invalid input parameter provided.".
5. Confirm the state side-effect of the wrong accept: GetDataAutomationLibraryIngestionJob on the returned jobArn resolves the job (jobStatus IN_PROGRESS, then COMPLETED about 3s later) and ListDataAutomationLibraryIngestionJobs for that library enumerates it — a job the real service never created.
6. After the fix, re-replay bedrock-data-automation-library-ingestion-job#task#2: the submission must be refused 400 ValidationException with message exactly "Invalid input parameter provided.", and no job may be minted or stored for it.
