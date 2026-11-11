Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-data-automation.ingestion-job-invalid-entity-type-rejected bedrock-data-automation.invoke-ingestion-job-wrong-arn-type-rejected
Issue-Shape-Id: com.amazonaws.bedrockdataautomation#AmazonBedrockKeystoneBuildTimeService
Issue-Key: bedrock-data-automation.ingestion-job-invalid-entity-type-rejected|run#11|AssertionError|assert not_found == "ResourceNotFoundException", f"rejected entityId should not exist: {not_found}"
Issue-Key: bedrock-data-automation.invoke-ingestion-job-wrong-arn-type-rejected|run#15|AssertionError|assert not_found == "ResourceNotFoundException", (
Issue-Commit: 4fbb8d92928479b0d33d95ed4fefcdea254a8e5f

# GetDataAutomationLibraryEntity answers 501 NotImplemented instead of resolving the entity or refusing 404 ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `4fbb8d92928479b0d33d95ed4fefcdea254a8e5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDataAutomationLibraryEntity

## Description
Cause 1 of 1: `GetDataAutomationLibraryEntity` is still an unimplemented scaffold stub, so every read of a library entity — present or absent — comes back as HTTP 501 `NotImplemented`. Two behavioral checks use this operation as the "effect absent" probe after a correctly refused ingestion job, and both get `NotImplemented` where AWS answers a modeled 404. Failing checks: `bedrock-data-automation.ingestion-job-invalid-entity-type-rejected`, `bedrock-data-automation.invoke-ingestion-job-wrong-arn-type-rejected`.

NOTE: the suite's auto-filing only marks a stub-blocked check when the 501 escapes as an error verdict. Here both checks catch the ClientError and assert on its code, so they are reported as `mismatch` and are NOT covered by any "ALREADY FILED" stub issue. The open stub issues for this service name `DeleteDataAutomationLibrary`, `InvokeDataAutomationLibraryIngestionJob`, `ListDataAutomationLibraryIngestionJobs`, `UpdateDataAutomationLibrary` and `ListDataAutomationLibraries` — none of them names `GetDataAutomationLibraryEntity`, and no open issue covers this operation.

AWS: Smithy (com.amazonaws.bedrockdataautomation#GetDataAutomationLibraryEntity, readonly, POST /data-automation-libraries/{libraryArn}/entityType/{entityType}/entities/{entityId}) models input `libraryArn` (DataAutomationLibraryArn, @required, @httpLabel, @length max 128, @pattern ^arn:aws(|-cn|-iso|-iso-[a-z]|-us-gov):bedrock:[a-zA-Z0-9-]*:[0-9]{12}:data-automation-library/[a-zA-Z0-9-]{12,36}$), `entityType` (EntityType enum: VOCABULARY | RECOGNIZED_PERSON, @required, @httpLabel) and `entityId` (EntityId, @required, @httpLabel, @length 1..128, @pattern ^[a-zA-Z0-9-_]+$); output `entity` (EntityDetails); errors AccessDeniedException (403), InternalServerException (500), ResourceNotFoundException (404), ThrottlingException (429), ValidationException (400). Doc: "Gets an existing entity based on entity type from the library"; ResourceNotFoundException: "This exception is thrown when a resource referenced by the operation does not exist". For a well-formed request naming an entityId that was never created in an existing library, AWS answers HTTP 404 with error code `ResourceNotFoundException`; the recorded message for this class is the fixed text "The specified entity ID and entity type is not found." (no variable parts — the requested entityId/entityType are not interpolated). For an entityId that does exist, AWS answers 200 with the `entity` member populated for the requested entityType. 501/`NotImplemented` is not a modeled outcome of this operation at all.

Emulator: every call returns HTTP 501 with error code `NotImplemented` and message "operation GetDataAutomationLibraryEntity is scaffolded but not implemented in this emulator". Replay output: `AssertionError: rejected entityId should not exist: NotImplemented` and `AssertionError: expected ResourceNotFoundException for the never-created entity, got 'NotImplemented'`.

Why: a client cannot read a library entity at all, and cannot distinguish "this entity does not exist" (404 ResourceNotFoundException, a normal control-flow outcome an SDK retry policy treats as terminal-and-expected) from "the service is broken". Idempotent upsert flows that probe for an entity before ingesting, and negative-path tests that confirm a rejected ingestion created nothing, both break: 501 is not in the operation's modeled error set, so generated clients surface it as an unmodeled/unknown failure rather than the typed ResourceNotFoundException.

Fix: implement the operation's handler — the `handle` body of `GetDataAutomationLibraryEntity`, replacing the `Scaffolded::not_implemented` return with a lookup of (libraryArn, entityType, entityId) in the service state's library-entity store: 200 + `entity` on hit, `ResourceNotFoundException` with the message above on miss, and `ResourceNotFoundException` when the libraryArn is well-formed but names no existing library. Modeled label constraints are enforced before the handler, so the handler only needs existence classification, not pattern validation.

Traces: bedrock-data-automation.ingestion-job-invalid-entity-type-rejected (the entityType-enum ValidationException and its message already pass; the check then fails on the absence probe); bedrock-data-automation.invoke-ingestion-job-wrong-arn-type-rejected (the libraryArn-pattern ValidationException already passes; the check then fails on the absence probe). Stub marker and handler: services/service-com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice/src/ops/get_data_automation_library_entity.rs line 4 (SCAFFOLDED marker) and line 169 (`Err(crate::scaffolded::Scaffolded::not_implemented("GetDataAutomationLibraryEntity"))`).

## Evidence
Expected (AWS): GetDataAutomationLibraryEntity(libraryArn=<existing library ARN>, entityType="VOCABULARY", entityId=<never-created id>) -> HTTP 404, Error.Code = "ResourceNotFoundException", Error.Message = "The specified entity ID and entity type is not found." (fixed text; no request values interpolated). For an existing entity -> HTTP 200 with the modeled `entity` (EntityDetails) member.

Actual (emulator): HTTP 501, Error.Code = "NotImplemented", Error.Message = "operation GetDataAutomationLibraryEntity is scaffolded but not implemented in this emulator" — for every input, existing or not.

Replay assertions:
  check_bedrock_data_automation_ingestion_job_invalid_entity_type_rejected.py:55 — AssertionError: rejected entityId should not exist: NotImplemented
  check_bedrock_data_automation_invoke_ingestion_job_wrong_arn_type_rejected.py:68 — AssertionError: expected ResourceNotFoundException for the never-created entity, got 'NotImplemented'

Source: services/service-com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice/src/ops/get_data_automation_library_entity.rs:169 returns Scaffolded::not_implemented; line 4 still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker.

## Steps to Reproduce
1. Boot the emulator and point a botocore `bedrock-data-automation` client at it.
2. library_arn = create_data_automation_library(libraryName=<unique>)['libraryArn'].
3. Call get_data_automation_library_entity(libraryArn=library_arn, entityType='VOCABULARY', entityId='never-created-id') — an entityId that satisfies ^[a-zA-Z0-9-_]+$ and was never ingested.
4. Observe HTTP 501 with Error.Code 'NotImplemented' instead of HTTP 404 with Error.Code 'ResourceNotFoundException' and message 'The specified entity ID and entity type is not found.'.
5. Equivalently, re-replay the behavioral checks bedrock-data-automation.ingestion-job-invalid-entity-type-rejected and bedrock-data-automation.invoke-ingestion-job-wrong-arn-type-rejected; both reach their ValidationException assertions successfully and then fail on this probe.
