Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: deadline.storage-profile-delete-blocked-by-queue-reference-rejected
Issue-Shape-Id: com.amazonaws.deadline#Deadline
Issue-Key: deadline.storage-profile-delete-blocked-by-queue-reference-rejected|run#8|AssertionError|assert control_error.get("Message") == f"Resource of type storage-profile with id {control_id} does not exist.", (
Issue-Commit: 88d6810d376793fb3a66f38c34bbaf625866946f

# deadline ResourceNotFoundException spells the storage-profile resource type camelCase ("storageProfile") instead of AWS's path-shaped "storage-profile"

> _Note: this issue was identified against the code base at commit `88d6810d376793fb3a66f38c34bbaf625866946f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetStorageProfile

## Description
Cause 1 of 1 — the resource-type token this service fills into its 404 (both into the message sentence and into the exception's `resourceType` member) is spelled in camelCase for storage profiles, so every storage-profile not-found refusal diverges from AWS by one word.

AWS: Smithy (services/service-com-amazonaws-deadline-deadline/smithy-model/com-amazonaws-deadline-deadline.json): GetStorageProfile is GET /2023-10-12/farms/{farmId}/storage-profiles/{storageProfileId}, success 200, and models ResourceNotFoundException with httpError 404 whose required members are message, resourceId and resourceType; the model documents resourceType only as "The type of the resource that couldn't be found" and enumerates no values, so the token is recorded behavior only. Recorded behavior: a GET for a syntactically valid (^sp-[0-9a-f]{32}$) storage-profile id the service does not hold — including one that was just deleted by DeleteStorageProfile (DELETE on the same URI, idempotent, success 200) — answers HTTP 404 ResourceNotFoundException with message 'Resource of type storage-profile with id <storageProfileId> does not exist.' and resourceType 'storage-profile'. Fixed text of the message: 'Resource of type <resourceType> with id <resourceId> does not exist.', trailing period included; its variable parts are the resource-kind token, which AWS spells the way the API path spells the resource ('storage-profile', matching the /storage-profiles/ URI segment), and the storageProfileId taken verbatim from the request. The sp- id in any one run is a per-run value, not part of the requirement.

Emulator: the status (404) and the error code (ResourceNotFoundException) are right and the sentence template is now right, but the resource-kind token is camelCase: the replayed check got message 'Resource of type storageProfile with id <sp-id> does not exist.' instead of 'Resource of type storage-profile with id <sp-id> does not exist.'. The same token is returned in the exception's resourceType member, so that member reads 'storageProfile' where AWS reports 'storage-profile'.

Why: an error message and the resourceType discriminator are part of the contract. A client, fixture, log scraper or support tool that matches AWS's sentence, or that switches on resourceType == 'storage-profile' to decide which resource was missing, finds no match and falls through to a generic error branch — so not-found handling (and delete/teardown loops that confirm removal by reading the exact AWS 404) that works against AWS breaks here, even though the status and code look correct.

Fix: the service's single-homed resource-type constant for storage profiles (the one the refusal builder is handed), not the refusal builder itself: the builder's sentence already matches AWS, and the token must be spelled as the path segment spells the resource. Its doc comment derives the token from the member name storageProfileId rather than from the /storage-profiles/ path segment the recordings show, which is the source of the defect. File and line citations are under Evidence.

Related but NOT covering this: open issue 20261030T195531Z-deadline-getstorageprofile-diverges-on-three-counts-epoch-seconds.md (wire corpus) bundled this token under its Cause 2 together with the old message template; the template part has since changed in the checkout while the token has not, and that issue carries no replay key for this behavioral check, so the surviving divergence is filed here. Open issue 20261029T172830Z-deadline-resourcenotfoundexception-message-is-the-emulator-s-own.md concerns the farm/queue/job message template, not this token.

Traces: deadline.storage-profile-delete-blocked-by-queue-reference-rejected.

## Evidence
Behavioral suite (simparity-behavioral-aws-v1) replayed against the emulator built from this checkout: deadline 11/22 passed; [mismatch] deadline.storage-profile-delete-blocked-by-queue-reference-rejected.

Failure detail, at the check's POSITIVE CONTROL (create an unreferenced storage profile, delete it, then read it back):
  check_deadline_storage_profile_delete_blocked_by_queue_reference_rejected.py line 34/36:
  assert control_error.get("Message") == f"Resource of type storage-profile with id {control_id} does not exist."
  AssertionError: expected the exact ResourceNotFoundException message, got 'Resource of type storageProfile with id sp-359b12bff2babdc41fa5208e03d09a6f does not exist.'
The Code assertion immediately before it (ResourceNotFoundException) PASSED, so the status and error code are correct and only the resource-kind token diverges. The sp- id shown is that run's generated value and is not part of the requirement.

Code evidence (current checkout):
- services/service-com-amazonaws-deadline-deadline/src/state.rs:180-185 —
    /// ... the member name `storageProfileId` names it `storageProfile`.
    pub const RESOURCE_TYPE_STORAGE_PROFILE: &str = "storageProfile";
  Compare the sibling tokens, e.g. src/state.rs:179 RESOURCE_TYPE_LIMIT = "limit".
- services/service-com-amazonaws-deadline-deadline/src/state.rs:1045-1051 — pub fn not_found(resource_type, resource_id) formats
    message: format!("Resource of type {resource_type} with id {resource_id} does not exist.")
  and copies resource_type into the exception's resourceType member, so the single constant decides both the sentence and the discriminator.

Model evidence: smithy_query(operation="GetStorageProfile") -> GET /2023-10-12/farms/{farmId}/storage-profiles/{storageProfileId}, errors include ResourceNotFoundException (httpError 404); smithy_query(operation="DeleteStorageProfile") -> DELETE on the same URI, idempotent, success 200.

## Steps to Reproduce
1. deadline.create_farm(displayName=...) -> farmId.
2. deadline.create_storage_profile(farmId=farmId, displayName=..., osFamily="LINUX") -> storageProfileId.
3. deadline.delete_storage_profile(farmId=farmId, storageProfileId=storageProfileId) (succeeds, 200).
4. deadline.get_storage_profile(farmId=farmId, storageProfileId=storageProfileId).
   Observed: HTTP 404, Code ResourceNotFoundException, Message 'Resource of type storageProfile with id <storageProfileId> does not exist.' and resourceType 'storageProfile'.
   Expected: HTTP 404, Code ResourceNotFoundException, Message 'Resource of type storage-profile with id <storageProfileId> does not exist.' and resourceType 'storage-profile'.
5. The same divergence appears for any syntactically valid ^sp-[0-9a-f]{32}$ id that was never created.

Re-replay check deadline.storage-profile-delete-blocked-by-queue-reference-rejected to confirm; note that fixing this token lets the check proceed past its positive control into the queue-reference (ConflictException) assertions, which this run never reached.
