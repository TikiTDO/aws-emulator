Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: deadline.get-farm-not-found-rejected
Issue-Shape-Id: com.amazonaws.deadline#Deadline
Issue-Key: deadline.get-farm-not-found-rejected|run#13|AssertionError|assert error.get("Message") == expected_message, (
Issue-Commit: 7b78f885090352093daa8ac758bd09ec0b97c628

# deadline ResourceNotFoundException message is the emulator's own wording ("The farm <id> can't be found.") instead of AWS's "Resource of type farm with id <id> does not exist."

> _Note: this issue was identified against the code base at commit `7b78f885090352093daa8ac758bd09ec0b97c628`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFarm

## Description
Cause 1 of 1 — the not-found refusal message for every resource this service addresses is reworded.
AWS: Smithy: GetFarm is GET /2023-10-12/farms/{farmId} with farmId @pattern ^farm-[0-9a-f]{32}$, and models ResourceNotFoundException (httpError 404); the model documents the exception only as "The requested resource can't be found." Recorded behavior: GetFarm on a syntactically valid but never-created farmId answers HTTP 404 ResourceNotFoundException with the message 'Resource of type farm with id farm-faa3d540be7279dd1ef170638c3e4d50 does not exist.' — fixed text 'Resource of type <resourceType> with id <resourceId> does not exist.', where <resourceType> is the resource kind the path addresses ('farm' here, matching the exception's own resourceType member) and <resourceId> is the id taken verbatim from the request. The trailing period is part of the fixed text.
Emulator: the same call answers 404 ResourceNotFoundException with 'The farm farm-faa3d540be7279dd1ef170638c3e4d50 can\u2019t be found.' — i.e. the template "The {resource_type} {resource_id} can't be found." produced by crate::state::not_found (services/service-com-amazonaws-deadline-deadline/src/state.rs:336-347). The status (404), the error code and the resourceId/resourceType members are right; only the message text diverges. Because that helper is single-homed, the same wrong wording is served for queues and jobs too (RESOURCE_TYPE_QUEUE / RESOURCE_TYPE_JOB, used from src/ops/get_job.rs:194-211 and its siblings).
Why: an error message is part of the contract. A client (or a test fixture, log scraper or support tool) that matches AWS's 'Resource of type X with id Y does not exist.' sees no match, so not-found handling that works against AWS silently falls through to a generic error branch here.
Fix: the crate's single-homed refusal wording — crate::state::not_found in src/state.rs (change the template, not the call sites), so every farm/queue/job 404 picks up AWS's sentence at once.
Traces: deadline.get-farm-not-found-rejected (the check's positive control, GetFarm on a just-created farm, passes, so only the message diverges).

## Evidence
Suite: [mismatch] deadline.get-farm-not-found-rejected — AssertionError: expected message 'Resource of type farm with id farm-faa3d540be7279dd1ef170638c3e4d50 does not exist.', got "The farm farm-faa3d540be7279dd1ef170638c3e4d50 can't be found." (the id shown is the per-run value the check derived by perturbing a real farmId; the requirement is the template, not that id.)

Code evidence: services/service-com-amazonaws-deadline-deadline/src/state.rs:337-347:
  pub fn not_found(resource_type: &str, resource_id: &str) -> ResourceNotFoundException {
      ... message: format!("The {resource_type} {resource_id} can't be found."), resource_id: ..., resource_type: ... }
Used for farm/queue/job alike (RESOURCE_TYPE_FARM/QUEUE/JOB, src/state.rs:41-46; call sites e.g. src/ops/get_job.rs:194-211).

Model evidence: GetFarm -> errors include ResourceNotFoundException with httpError 404.

## Steps to Reproduce
1. deadline.create_farm(displayName=...) -> farmId (positive control: deadline.get_farm(farmId=farmId) succeeds).
2. Perturb that id into a syntactically valid but never-created farmId (same ^farm-[0-9a-f]{32}$ shape) and call deadline.get_farm(farmId=<that id>).
3. Observed: HTTP 404 ResourceNotFoundException, Message "The farm <id> can't be found." Expected: HTTP 404 ResourceNotFoundException, Message 'Resource of type farm with id <id> does not exist.'
4. The same divergence appears for queue and job 404s (e.g. get_job with an unknown jobId), since one helper renders them all.
