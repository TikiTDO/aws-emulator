Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: deadline.storage-profile-delete-blocked-by-queue-reference-rejected
Issue-Shape-Id: com.amazonaws.deadline#Deadline
Issue-Key: deadline.storage-profile-delete-blocked-by-queue-reference-rejected|run#14|AssertionError|assert blocked_error.get("Code") == "ConflictException", f"expected ConflictException, got {blocked_error}"
Issue-Commit: 1228e4d90eea75353d965a6d30fc01efaf5f35ba

# deadline DeleteStorageProfile on a profile referenced by a queue returns ValidationException (400) instead of ConflictException (409)

> _Note: this issue was identified against the code base at commit `1228e4d90eea75353d965a6d30fc01efaf5f35ba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteStorageProfile

## Description
Cause 1 of 1: DeleteStorageProfile refuses a delete that is blocked by a queue's allowedStorageProfileIds reference with the wrong error class, error code and error message.

AWS: Deadline refuses DeleteStorageProfile while the profile is still named in some queue's allowedStorageProfileIds. Recorded behavior (the Smithy model lists only AccessDeniedException/InternalServerErrorException/ThrottlingException/ValidationException on DeleteStorageProfile, so the refusal class comes from the recording): HTTP 409, error code ConflictException, message exactly "Request could not be completed: Storage profile is associated with one or more Queues." — that message is entirely fixed text for this error class; it interpolates no request or state value (no profile id, no queue id). Smithy: com.amazonaws.deadline#ConflictException has @httpError 409 and required members message, reason (ConflictExceptionReason, one of CONFLICT_EXCEPTION/CONCURRENT_MODIFICATION/RESOURCE_ALREADY_EXISTS/RESOURCE_IN_USE/STATUS_CONFLICT), resourceId and resourceType, plus optional context; a conformant refusal therefore also carries reason=RESOURCE_IN_USE with the blocked profile's id/type, not just a message. The surrounding steps of the same check confirm the rest of the contract AWS honours and the emulator must keep honouring: the referenced profile survives the rejected delete, and the identical delete succeeds once UpdateQueue removes the reference.

Emulator: the call is refused, but as HTTP 400 ValidationException with the message "The storage profile <sp-id> can't be deleted because it is associated with the queue <queue-id>." (the two ids are interpolated from the request/state — in the replay they were sp-782977f832c093976a58c1af60f3ea67 and queue-ad5240be235f2567dd5d660f4edba505, which are per-run values, not part of any template). So three things diverge at once: status 400 vs 409, code ValidationException vs ConflictException, and a locally-invented message instead of AWS's fixed sentence; the ConflictException-only members (reason, resourceId, resourceType) are absent entirely.

Why: a client that distinguishes a retry-after-mutation conflict from a malformed request gets it backwards. Callers catch deadline.exceptions.ConflictException (or branch on Error.Code == "ConflictException", or on 409) to mean "a dependent resource still holds this one — detach it and retry"; a 400 ValidationException reads as "the request itself is invalid", so such code aborts, surfaces a bogus validation error to the user, or classifies the failure as non-retryable. Teardown and reconciliation loops are hit hardest: the suite's own cleanup helper retries on ConflictException and re-raises anything else, which is exactly the shape of real-world dependency-ordered cleanup, and it breaks against this emulator. Clients that parse reason/resourceId/resourceType to find what is holding the profile get nothing to parse.

Fix: the refusal path of the DeleteStorageProfile handler (services/service-com-amazonaws-deadline-deadline/src/ops/delete_storage_profile.rs and whatever shared error-construction it calls) must raise the service's ConflictException — 409, code ConflictException, the fixed message above, reason RESOURCE_IN_USE and the profile's resourceId/resourceType — in place of the ValidationException it builds today. The dependency check itself (scanning queues' allowedStorageProfileIds) is already correct and must stay, as must leaving the profile intact and allowing the delete once the reference is removed.

Traces: failing check deadline.storage-profile-delete-blocked-by-queue-reference-rejected, whose positive control (an unreferenced profile deletes cleanly and then 404s as ResourceNotFoundException) passes, so only the blocked-delete refusal diverges. Related but distinct open issue, which does NOT cover this: 20261112T132224Z-deadline-resourcenotfoundexception-spells-the-storage-profile-resource-type.md (ResourceNotFoundException resource-type spelling).

## Evidence
Check: deadline.storage-profile-delete-blocked-by-queue-reference-rejected (verdict: mismatch)

Assertion that failed (check_deadline_storage_profile_delete_blocked_by_queue_reference_rejected.py line 57):
  assert blocked_error.get("Code") == "ConflictException", f"expected ConflictException, got {blocked_error}"
AssertionError: expected ConflictException, got {'Message': "The storage profile sp-782977f832c093976a58c1af60f3ea67 can't be deleted because it is associated with the queue queue-ad5240be235f2567dd5d660f4edba505.", 'Code': 'ValidationException'}

Expected (AWS, as recorded and as the check asserts):
  HTTP 409
  Error.Code    = "ConflictException"
  Error.Message = "Request could not be completed: Storage profile is associated with one or more Queues."   (fixed text, no interpolated values)
  plus ConflictException's required reason / resourceId / resourceType members (reason RESOURCE_IN_USE)

Actual (emulator):
  HTTP 400
  Error.Code    = "ValidationException"
  Error.Message = "The storage profile <storageProfileId> can't be deleted because it is associated with the queue <queueId>."  (ids interpolated from request/state)
  no reason / resourceId / resourceType members

Model evidence (smithy_query on services/service-com-amazonaws-deadline-deadline/smithy-model/com-amazonaws-deadline-deadline.json):
  DeleteStorageProfile: DELETE /2023-10-12/farms/{farmId}/storage-profiles/{storageProfileId}, idempotent, errors = [AccessDeniedException 403, InternalServerErrorException 500, ThrottlingException 429, ValidationException 400]
  ConflictException: @error(client) @httpError(409); required message, reason (ConflictExceptionReason), resourceId, resourceType; optional context
  ConflictExceptionReason values: CONFLICT_EXCEPTION, CONCURRENT_MODIFICATION, RESOURCE_ALREADY_EXISTS, RESOURCE_IN_USE, STATUS_CONFLICT

## Steps to Reproduce
1. deadline.CreateFarm(displayName=<unique>) -> farmId
2. deadline.CreateStorageProfile(farmId, displayName=<unique>, osFamily="LINUX") -> storageProfileId
3. deadline.CreateQueue(farmId, displayName=<unique>, allowedStorageProfileIds=[storageProfileId]) -> queueId
4. deadline.DeleteStorageProfile(farmId, storageProfileId)
   -> AWS: HTTP 409, Code=ConflictException, Message="Request could not be completed: Storage profile is associated with one or more Queues."
   -> Emulator: HTTP 400, Code=ValidationException, Message="The storage profile <sp-id> can't be deleted because it is associated with the queue <queue-id>."
5. deadline.GetStorageProfile(farmId, storageProfileId) still returns the profile (this part matches AWS and must not regress).
6. deadline.UpdateQueue(farmId, queueId, allowedStorageProfileIdsToRemove=[storageProfileId]), then repeat step 4: the delete must now succeed and GetStorageProfile must then fail with ResourceNotFoundException "Resource of type storage-profile with id <sp-id> does not exist."

Replay with: run_behavioral_suite, check deadline.storage-profile-delete-blocked-by-queue-reference-rejected
