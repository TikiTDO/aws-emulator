Issue-Source: trace-sweep-divergence
Issue-Trace: workspaces-create-and-migrate-workspace-to-new-bundle
Issue-Step: setup#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: workspaces-create-and-migrate-workspace-to-new-bundle workspaces-create-and-rebuild-workspace workspaces-create-and-restore-workspace workspaces-create-primary-and-standby-workspace-dr workspaces-operate-on-terminated-workspace-errors workspaces-create-workspaces-batch-start-stop-reboot
Issue-Shape-Id: com.amazonaws.workspaces#WorkspacesService
Issue-Key: workspaces|CreateWorkspaces|2xx->2xx|$.FailedRequests[*].WorkspaceRequest.WorkspaceProperties.RecycleMode
Issue-Key: workspaces|CreateWorkspaces|2xx->2xx|$.FailedRequests[*].ErrorCode
Issue-Key: workspaces|CreateWorkspaces|2xx->2xx|$.FailedRequests[*].ErrorMessage
Issue-Commit: 600bb9247975905af81e2aa2cda8a015f9396423

# CreateWorkspaces FailedRequests entries diverge: request echo drops WorkspaceProperties members and a missing bundle is reported as a missing directory

> _Note: this issue was identified against the code base at commit `600bb9247975905af81e2aa2cda8a015f9396423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWorkspaces

## Description
Two distinct causes in the WorkSpaces CreateWorkspaces response path. Both corrupt the FailedRequests[] entries the emulator returns for requests it rejects: one loses part of the echoed WorkspaceRequest, the other mis-attributes WHICH referenced resource was not found. 2 causes total; both must be closed.

Cause 1 of 2 — the echoed WorkspaceRequest.WorkspaceProperties loses members the caller submitted (fixture workspaces-create-and-migrate-workspace-to-new-bundle, step setup#3).
AWS: recorded behavior only. The on-disk Smithy model (services/service-com-amazonaws-workspaces-workspacesservice) defines FailedCreateWorkspaceRequest with members WorkspaceRequest (target WorkspaceRequest), ErrorCode (ErrorType) and ErrorMessage (Description), and WorkspaceRequest.WorkspaceProperties targets WorkspaceProperties; the model carries no RecycleMode member at all (smithy_query shape=RecycleMode -> "shape 'RecycleMode' not found"). The recordings show AWS echoing the submitted WorkspaceRequest back in each FailedRequests entry and that echo carrying WorkspaceProperties.RecycleMode with the value the request supplied ('DISABLED' in these recordings). The requirement is the general one: every WorkspaceProperties member present in the submitted request appears, unchanged, in the FailedRequests[i].WorkspaceRequest echo for that request — the echo is a verbatim copy of the input element, not a projection of it. The literal 'DISABLED' is this recording's input value filling that template, not a constant to hard-code.
Emulator: the member is absent from the echo — replay reports "$.FailedRequests[0].WorkspaceRequest.WorkspaceProperties.RecycleMode: missing (expected 'DISABLED', got None)", with HTTP 200 on both sides and every other echoed member matching. In the batch fixture the same loss occurs on FailedRequests[1] as well, so it is per-entry and not specific to the first element.
Hypothesis: the echo is rebuilt by re-serializing the emulator's parsed WorkspaceProperties struct, so any member the emulator's model does not carry is silently dropped on the way out rather than round-tripped.
Why: a client that submits a batch and reads FailedRequests to decide what to resubmit gets back a WorkspaceRequest that is not the one it sent. Resubmitting the echoed element re-creates the WorkSpace without the dropped properties, and any client that diffs the echo against its own request (a common idempotency/retry pattern) sees a spurious difference and treats the response as inconsistent.
Fix: the CreateWorkspaces handler's response serializer for FailedRequests — the echo must preserve the submitted request element as received instead of projecting it through the modeled WorkspaceProperties members; this also needs RecycleMode to exist end-to-end (parse, state, serialize) in the workspaces service crate.

Cause 2 of 2 — a request naming a non-existent bundle is failed as a non-existent directory (fixture workspaces-create-workspaces-batch-start-stop-reboot, step task#3).
AWS: recorded behavior only for the code strings; the model types FailedCreateWorkspaceRequest.ErrorCode as the open string ErrorType, so the per-resource codes are not enumerated there, and WorkspaceRequest requires both DirectoryId (pattern ^(d-[0-9a-f]{8,63}$)|(wsd-[0-9a-z]{8,63}$)$) and BundleId (pattern ^wsb-[0-9a-z]{8,63}$). The recording shows AWS resolving each referenced resource independently and reporting the one that is actually absent: for an element whose directory exists but whose bundle does not, the entry carries ErrorCode 'ResourceNotFound.Bundle' and ErrorMessage 'The specified bundle could not be found in the specified region.' Both strings are entirely fixed text for this error class — the message interpolates nothing from the request — and the parallel directory case is the fixed text 'The specified directory could not be found in the specified region.' under 'ResourceNotFound.Directory'. HTTP status is 200 in both cases: these are per-element failures inside the batch, not a top-level error.
Emulator: for that element it returns ErrorCode 'ResourceNotFound.Directory' and ErrorMessage 'The specified directory could not be found in the specified region.' — the wrong resource kind, with the status (200) and the entry's position correct.
Hypothesis: the handler validates the directory reference first and, on any unresolved reference, emits the directory-not-found pair without ever testing the bundle id against bundle state — so the bundle-missing case is reported under the directory code.
Why: 'ResourceNotFound.Bundle' and 'ResourceNotFound.Directory' are the only machine-readable signal a batch caller has for which field of the element to correct. A client is told its directory is gone when the directory is healthy, so it re-registers or re-creates a directory (or aborts the whole batch as an environment failure) instead of fixing the one bad BundleId, and the actual bad bundle reference is never surfaced.
Fix: the CreateWorkspaces handler's per-element validation in the workspaces service crate — resolve DirectoryId and BundleId against their own state and emit the ResourceNotFound.<Resource> code and message for the reference that failed to resolve, rather than a single directory-shaped failure for every unresolved reference.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200; 83 steps, 46 passed).

Cause 1 — CreateWorkspaces, expected status 200, actual 200:
  $.FailedRequests[0].WorkspaceRequest.WorkspaceProperties.RecycleMode: missing (expected 'DISABLED', got None)
Same cluster affects 5 more steps: workspaces-create-and-migrate-workspace-to-new-bundle#setup#3, workspaces-create-and-rebuild-workspace#task#3, workspaces-create-and-restore-workspace#setup#3, workspaces-create-primary-and-standby-workspace-dr#setup#3, workspaces-operate-on-terminated-workspace-errors#setup#3.

Cause 2 — CreateWorkspaces, expected status 200, actual 200 (fixture workspaces-create-workspaces-batch-start-stop-reboot, step task#3):
  $.FailedRequests[0].WorkspaceRequest.WorkspaceProperties.RecycleMode: missing (expected 'DISABLED', got None)
  $.FailedRequests[1].ErrorCode: value_mismatch (expected 'ResourceNotFound.Bundle', got 'ResourceNotFound.Directory')
  $.FailedRequests[1].ErrorMessage: value_mismatch (expected 'The specified bundle could not be found in the specified region.', got 'The specified directory could not be found in the specified region.')
  $.FailedRequests[1].WorkspaceRequest.WorkspaceProperties.RecycleMode: missing (expected 'DISABLED', got None)

Model evidence (services/service-com-amazonaws-workspaces-workspacesservice/smithy-model/com-amazonaws-workspaces-workspacesservice.json): CreateWorkspaces input Workspaces (WorkspaceRequestList, required, @length 1..25), output FailedRequests (FailedCreateWorkspaceRequests) + PendingRequests; FailedCreateWorkspaceRequest = {WorkspaceRequest, ErrorCode: ErrorType (open string), ErrorMessage: Description}; WorkspaceRequest requires DirectoryId + UserName + BundleId; WorkspaceProperties has no RecycleMode member.

## Steps to Reproduce
1. git clone the repo, build and boot the emulator, confirm /__admin/health answers 200.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Cause 1: replay fixture workspaces-create-and-migrate-workspace-to-new-bundle step setup#3 (CreateWorkspaces with a batch element whose WorkspaceProperties sets RecycleMode and which is rejected). Compare FailedRequests[0].WorkspaceRequest to the submitted element: the emulator's echo omits WorkspaceProperties.RecycleMode that the request supplied. The same loss reproduces on workspaces-create-and-rebuild-workspace#task#3, workspaces-create-and-restore-workspace#setup#3, workspaces-create-primary-and-standby-workspace-dr#setup#3 and workspaces-operate-on-terminated-workspace-errors#setup#3.
4. Cause 2: replay fixture workspaces-create-workspaces-batch-start-stop-reboot step task#3 (CreateWorkspaces batch whose second element names an existing directory and a bundle id that does not exist). Read FailedRequests[1]: the emulator returns ErrorCode 'ResourceNotFound.Directory' / 'The specified directory could not be found in the specified region.' where AWS returns 'ResourceNotFound.Bundle' / 'The specified bundle could not be found in the specified region.' (HTTP 200 either way).

## Resolution

Cause 1 — FIXED (key `$.FailedRequests[*].WorkspaceRequest.WorkspaceProperties.RecycleMode`,
6 units). `WorkspaceProperties` now DECLARES `RecycleMode`, so the member exists end to end
— parsed from the request, persisted on the record, reported by every operation that reads
that shape. The declaration is this crate's own, generated from the small overlay model
`smithy-model/local-additions.json` rather than by editing the published snapshot, which is
kept as shipped (`smithy-model/LOCAL-ADDITIONS.md` says what is declared locally and why).
Beside that, the member joins the EFFECTIVE property set
(`state::effective_workspace_properties`): a submitted value is echoed and stored verbatim,
and a request that omits it gets `state::DEFAULT_RECYCLE_MODE`, the value a recorded AWS
response reports for a request that carried no `WorkspaceProperties` at all — the same
resolution `RunningMode` already had. Pinned by
`tests/all/workspace_properties_local_members.rs`, which drives the whole declared member
set through the wire and fails if the snapshot is ever refreshed with the member or with one
the local declaration does not carry.

Cause 2 — DECLINED (keys `$.FailedRequests[*].ErrorCode` and
`$.FailedRequests[*].ErrorMessage`, one unit:
workspaces-create-workspaces-batch-start-stop-reboot). The emulator already resolves each
reference independently and reports the one that failed, with AWS's own fixed sentence for
each class — `ResourceNotFound.Bundle` / "The specified bundle could not be found in the
specified region." for an unheld `BundleId`, `ResourceNotFound.Directory` / "The specified
directory could not be found in the specified region." for an unregistered `DirectoryId` —
which `create_workspaces_effective_properties::a_refusal_names_the_resource_that_failed_to_resolve`
pins over a table, message included. Neither wording differs from AWS; what differs for that
one element is which class applies. Measured on this checkout by resolving the bundle before
the directory and replaying: the recorded element then answers `Bundle`, but the element the
other five fixtures record as `Directory` answers `Bundle` too — so in the emulator BOTH
references of all these elements are unresolvable, and the recordings ask for two different
answers to the same resolution result. The only input left to tell them apart is the
recorded identifier strings themselves: AWS-generated, per-run ids (a Directory Service
directory, an Amazon catalog bundle) that no step of these fixtures creates in the emulator.
Matching them would take a case keyed to the recorded input, so the order stays the model's
own member order (`DirectoryId` before `BundleId`) — the answer five of the six recordings
corroborate.
