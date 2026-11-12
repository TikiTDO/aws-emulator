Issue-Source: trace-sweep-divergence
Issue-Trace: deadline-storage-profile-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: deadline-storage-profile-lifecycle
Issue-Shape-Id: com.amazonaws.deadline#Deadline
Issue-Key: deadline|GetStorageProfile|2xx->2xx|$.createdAt
Issue-Key: deadline|GetStorageProfile|4xx->4xx|$.message
Issue-Key: deadline|GetStorageProfile|4xx->4xx|$.resourceType
Issue-Key: deadline|GetStorageProfile|4xx->2xx|$.message
Issue-Key: deadline|GetStorageProfile|4xx->2xx|$.resourceId
Issue-Key: deadline|GetStorageProfile|4xx->2xx|$.resourceType
Issue-Key: deadline|GetStorageProfile|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: deadline|GetStorageProfile|4xx->2xx|status
Issue-Commit: 610eb4b5abbe12768656cecbe381c20cd7e570c7

# deadline GetStorageProfile diverges on three counts: epoch-seconds createdAt, reworded 404, and a deleted profile still readable

> _Note: this issue was identified against the code base at commit `610eb4b5abbe12768656cecbe381c20cd7e570c7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetStorageProfile

## Description
Three independent defects on the deadline storage-profile read path, all observed replaying the storage-profile lifecycle fixture. A fixer must close ALL THREE before submitting.

Cause 1 of 3 — createdAt is serialized as an epoch-seconds NUMBER instead of a date-time string (fixture deadline-storage-profile-lifecycle, step task#3).
AWS: Smithy: GetStorageProfile is GET /2023-10-12/farms/{farmId}/storage-profiles/{storageProfileId} (success 200); its output member createdAt targets CreatedAt, is @required, type timestamp, timestampFormat "date-time" (updatedAt likewise). The service speaks restJson1, so a date-time timestamp goes on the wire as an ISO-8601/RFC-3339 STRING (shape "<yyyy>-<MM>-<dd>T<HH>:<mm>:<ss>Z"), never a JSON number. The recorded response asserts that member by SHAPE, not by value (per-run instants are not a requirement).
Emulator: the 200 body carries $.createdAt as a bare JSON number of epoch seconds with a fractional part (the replay shows 1793389995.5114884 for that run). All other asserted members of the 200 match.
Why: every AWS SDK deserializes this member per the model, so a number where a date-time string is modeled either fails to parse or is silently reinterpreted; any client doing date math, display or comparison on createdAt/updatedAt gets a wrong or absent instant.
Fix: the operation's response serialization of timestamp members — the generated output schema / serializer timestampFormat wiring, not the handler: the handler already builds a correctly typed value. Hypothesis: the serializer falls back to restJson1's protocol-default epoch-seconds encoding because the model's @timestampFormat("date-time") on CreatedAt/UpdatedAt is not honored at serialization time.
Traces: deadline-storage-profile-lifecycle#task#3.

Cause 2 of 3 — the storage-profile 404 uses the emulator's own wording and a camelCase resourceType (fixture deadline-storage-profile-lifecycle, step task#4).
AWS: Smithy: GetStorageProfile models ResourceNotFoundException (httpError 404) whose required members are message, resourceId and resourceType; the model documents resourceType only as "The type of the resource that couldn't be found" and enumerates no values. Recorded behavior: a GET for a syntactically valid (^sp-[0-9a-f]{32}$) but non-existent storage profile answers HTTP 404 ResourceNotFoundException with message 'Resource of type storage-profile with id <storageProfileId> does not exist.' and resourceType 'storage-profile'. Fixed text: 'Resource of type <resourceType> with id <resourceId> does not exist.' including the trailing period; the variable parts are the resource kind as the API path spells it ('storage-profile', matching the URI segment /storage-profiles/) and the storageProfileId taken verbatim from the request. The recorded resourceId matched and is not part of this finding.
Emulator: same status (404) and error code, but message "The storageProfile <id> can't be found." and resourceType 'storageProfile' — i.e. the template "The {resource_type} {resource_id} can't be found." rendered from the camelCase token.
Why: an error message and the resourceType discriminator are part of the contract. A client, fixture or support tool that matches AWS's sentence or switches on resourceType == 'storage-profile' sees no match and falls through to a generic error branch, so not-found handling that works against AWS breaks here.
Fix: the service's single-homed refusal builder (crate::state::not_found — change the template, not its call sites) and the storage-profile resourceType constant, which must spell the kebab-case path token. Related but NOT covering this: open issue 20261029T172830Z-deadline-resourcenotfoundexception-message-is-the-emulator-s-own filed the same wording defect for GetFarm from the behavioral suite; it carries no replay key for this fixture step and says nothing about the wrong resourceType token, so this cause is filed here.
Traces: deadline-storage-profile-lifecycle#task#4 (see Evidence for file/line citations).

Cause 3 of 3 — a storage profile remains readable after it has been deleted: 200 where AWS answers 404 (fixture deadline-storage-profile-lifecycle, step task#6).
AWS: Smithy: DeleteStorageProfile is DELETE /2023-10-12/farms/{farmId}/storage-profiles/{storageProfileId} (idempotent, success 200, empty output); once it succeeds the profile no longer exists, so GetStorageProfile on that id answers HTTP 404 ResourceNotFoundException — body with message 'Resource of type storage-profile with id <storageProfileId> does not exist.', resourceType 'storage-profile', resourceId the id from the request — and the restJson1 error header x-amzn-errortype naming the exception. That 404 is what the recording shows for the read that follows the delete.
Emulator: the same read answers HTTP 200 with the profile body still intact: no message, no resourceType, no resourceId, and no x-amzn-errortype header. The delete left the profile in state.
Why: a client cannot tell a deleted resource from a live one. Deletion appears to silently fail, so cleanup/teardown loops never converge, re-creation and uniqueness checks see a phantom resource, and any workflow that polls for 404 after delete hangs.
Fix: the delete path's state mutation — the profile must be removed from its farm's storage-profile map so the read path's existing not-found branch fires. Note DeleteStorageProfile is still an unimplemented scaffold stub in this checkout (open issue 20261030T150940Z-deadline-deletestorageprofile-stub.md owns standing the operation up); implementing it must also make this subsequent read answer the modeled 404, which is the divergence recorded here.
Traces: deadline-storage-profile-lifecycle#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, score 45.1%):

Cause 1 — fixture deadline-storage-profile-lifecycle step task#3, GetStorageProfile, expected 200 / actual 200: "$.createdAt: value_mismatch (expected '<shaped-not-asserted>', got 1793389995.5114884)". The expected value is asserted by shape (a date-time string); the emulator emitted a JSON number. Model evidence: smithy_query(operation="GetStorageProfile") -> output createdAt {target CreatedAt, required, type timestamp, timestampFormat "date-time"}, updatedAt likewise. Code: services/service-com-amazonaws-deadline-deadline/src/ops/get_storage_profile.rs:187-189 sets created_at: crate::state::timestamp(profile.created_at), and src/state.rs:613-616 returns an aws_smithy_types::DateTime — so the date-time intent is lost downstream of the handler, in serialization.

Cause 2 — fixture deadline-storage-profile-lifecycle step task#4, GetStorageProfile, expected 404 / actual 404:
  "$.message: value_mismatch (expected 'Resource of type storage-profile with id sp-a518ffae0af04b327a6937922fa90e12 does not exist.', got \"The storageProfile sp-a518ffae0af04b327a6937922fa90e12 can't be found.\")"
  "$.resourceType: value_mismatch (expected 'storage-profile', got 'storageProfile')"
The sp- id shown is that run's value and is not a requirement; the requirements are the sentence template and the kebab-case resourceType token. Code: services/service-com-amazonaws-deadline-deadline/src/state.rs:618-629 — not_found() formats message: format!("The {resource_type} {resource_id} can't be found."); src/state.rs:96-101 — RESOURCE_TYPE_STORAGE_PROFILE: &str = "storageProfile" (its comment derives the token from the member name storageProfileId rather than from the /storage-profiles/ path segment the recording shows). Call site: src/ops/get_storage_profile.rs:173-181. Model evidence: smithy_query(shape="ResourceNotFoundException") -> httpError 404, required members message/resourceId/resourceType.

Cause 3 — fixture deadline-storage-profile-lifecycle step task#6, GetStorageProfile, expected 404 / actual 200:
  "status: expected 404, got 200"
  "$.message: missing (expected 'Resource of type storage-profile with id sp-5c5932ea5a3cca06e6a2f0c5eec31817 does not exist.', got None)"
  "$.resourceType: missing (expected 'storage-profile', got None)"
  "$.resourceId: missing (expected 'sp-5c5932ea5a3cca06e6a2f0c5eec31817', got None)"
  "$headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)"
Model evidence: smithy_query(operation="DeleteStorageProfile") -> DELETE /2023-10-12/farms/{farmId}/storage-profiles/{storageProfileId}, idempotent, success 200; GetStorageProfile models ResourceNotFoundException (404). Code: the read's not-found branch exists (src/ops/get_storage_profile.rs:173-181) and fires only if the profile is absent from ComAmazonawsDeadlineDeadlineState's farm -> storage_profiles map (src/state.rs:570-582), so the gap is the removal on delete.

## Steps to Reproduce
Replay fixture deadline-storage-profile-lifecycle of trace set simparity-wire-aws-v1 against the emulator, or by hand:

1. deadline.create_farm(displayName=...) -> farmId; deadline.create_storage_profile(farmId=farmId, displayName=..., osFamily="LINUX") -> storageProfileId.
2. (Cause 1, step task#3) deadline.get_storage_profile(farmId=farmId, storageProfileId=storageProfileId). Observed: 200 with "createdAt": 1793389995.5114884 (a JSON number of epoch seconds). Expected: 200 with createdAt as an ISO-8601 date-time string, per @timestampFormat("date-time").
3. (Cause 2, step task#4) deadline.get_storage_profile(farmId=farmId, storageProfileId=<a syntactically valid ^sp-[0-9a-f]{32}$ id that was never created>). Observed: 404 ResourceNotFoundException, message "The storageProfile <id> can't be found.", resourceType "storageProfile". Expected: 404 ResourceNotFoundException, message 'Resource of type storage-profile with id <id> does not exist.', resourceType "storage-profile" (resourceId already correct).
4. (Cause 3, step task#6) deadline.delete_storage_profile(farmId=farmId, storageProfileId=storageProfileId), then deadline.get_storage_profile(farmId=farmId, storageProfileId=storageProfileId). Observed: the read answers 200 with the profile body, no error members and no x-amzn-errortype header. Expected: 404 ResourceNotFoundException with message 'Resource of type storage-profile with id <id> does not exist.', resourceType "storage-profile", resourceId <id>, and an x-amzn-errortype header. (DeleteStorageProfile is currently an unimplemented stub, so the delete itself answers 501 here — standing it up must also make this read 404.)
