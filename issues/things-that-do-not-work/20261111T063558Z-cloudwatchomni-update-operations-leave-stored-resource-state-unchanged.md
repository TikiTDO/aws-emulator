Issue-Source: trace-sweep-divergence
Issue-Trace: cloudwatchomni-access-profile-and-grant-lifecycle
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudwatchomni-access-profile-and-grant-lifecycle cloudwatchomni-omni-dashboard-update-and-validation cloudwatchomni-view-update-and-conflict-errors
Issue-Shape-Id: com.amazonaws.cloudwatchomni#CloudWatchOmniFrontend
Issue-Key: cloudwatchomni|GetAccessProfile|2xx->2xx|$.accessProfile.arn
Issue-Key: cloudwatchomni|GetAccessProfile|2xx->2xx|$.accessProfile.assumeStatus
Issue-Key: cloudwatchomni|GetAccessProfile|2xx->2xx|$.accessProfile.createdAt
Issue-Key: cloudwatchomni|GetAccessProfile|2xx->2xx|$.accessProfile.description
Issue-Key: cloudwatchomni|GetAccessProfile|2xx->2xx|$.accessProfile.updatedAt
Issue-Key: cloudwatchomni|GetOmniDashboard|2xx->2xx|$.omniDashboard.createdAt
Issue-Key: cloudwatchomni|GetOmniDashboard|2xx->2xx|$.omniDashboard.description
Issue-Key: cloudwatchomni|GetOmniDashboard|2xx->2xx|$.omniDashboard.tags
Issue-Key: cloudwatchomni|GetOmniDashboard|2xx->2xx|$.omniDashboard.updatedAt
Issue-Key: cloudwatchomni|GetView|2xx->2xx|$.createdAt
Issue-Key: cloudwatchomni|GetView|2xx->2xx|$.description
Issue-Key: cloudwatchomni|GetView|2xx->2xx|$.updatedAt
Issue-Commit: 826b08334102dd43451716f9b2396fd89c307280

# cloudwatchomni Update operations leave stored resource state unchanged

> _Note: this issue was identified against the code base at commit `826b08334102dd43451716f9b2396fd89c307280`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccessProfile

## Description
In three fixtures a resource is created with description 'first', updated to description 'second', then read back. On AWS the read returns the updated value; the emulator returns the pre-update value, so the mutation never reached its state. This bundle has THREE causes; all three must be fixed and re-replayed.

Cause 1 of 3 - GetAccessProfile returns the pre-update description and a wrong assumeStatus (fixture cloudwatchomni-access-profile-and-grant-lifecycle, step task#6).
AWS: Smithy UpdateAccessProfile is documented 'Updates the name or description of an access profile. Only the provided fields are changed; omitted fields are left unchanged' and returns the updated AccessProfile; GetAccessProfile 'Retrieves an access profile by ID. The response indicates whether the calling principal is currently authorized to assume it' via AccessProfile.assumeStatus (enum AssumeStatus). The recording shows the read-after-update returning description = the value sent in the update, and assumeStatus = ALLOWED for the account-owning caller that created the profile.
Emulator: the Get after the update returns $.accessProfile.description still at the created value, and $.accessProfile.assumeStatus = DENIED where AWS returns ALLOWED. (It also carries the space-scoped ARN and numeric timestamps covered by the response-shaping bundle.)
Why: a client that updates a profile and re-reads it to confirm sees its write silently lost, and a console-style client gating the 'assume profile' action on assumeStatus hides a capability the caller actually has.
Fix: the UpdateAccessProfile handler's state write (the mutation must be committed to the profile record Get reads) and the GetAccessProfile handler's assumeStatus derivation for the owning caller.
Traces: cloudwatchomni-access-profile-and-grant-lifecycle#task#6.

Cause 2 of 3 - GetOmniDashboard returns the pre-update description (fixture cloudwatchomni-omni-dashboard-update-and-validation, step task#7).
AWS: Smithy UpdateOmniDashboard - 'Updates an existing dashboard within a space. Only the provided fields are changed; omitted fields are left unchanged', output omniDashboard (OmniDashboard, @required). The recording's read-after-update returns the updated description.
Emulator: $.omniDashboard.description is still the created value after the update step; tags and timestamps also diverge as per the response-shaping bundle. The open issue 20261111T022337Z-cloudwatchomni-updateomnidashboard-stub.md reports UpdateOmniDashboard as an unimplemented scaffold; closing that stub is a prerequisite but does not cover this read-back divergence, which must be re-replayed on its own.
Why: the dashboard a client renders after a successful update is stale, so edits appear to be discarded.
Fix: the UpdateOmniDashboard handler and the dashboard record in the cloudwatchomni state store.
Traces: cloudwatchomni-omni-dashboard-update-and-validation#task#7.

Cause 3 of 3 - GetView returns the pre-update description (fixture cloudwatchomni-view-update-and-conflict-errors, step task#5).
AWS: Smithy UpdateView - 'Updates an existing view's definition and/or description. Only the fields you provide are changed', @idempotent, output includes description and updatedAt; GetView 'Returns the definition and metadata of the specified view'. The recording's read-after-update returns the updated description.
Emulator: $.description is still the created value after the update; createdAt/updatedAt are numeric as per the response-shaping bundle. The open issue 20261111T022337Z-cloudwatchomni-updateview-stub.md reports UpdateView as an unimplemented scaffold; it is a prerequisite, not coverage for this step.
Why: a client that edits a saved view and re-reads it sees the old definition/description, and downstream telemetry queries referencing the view run against the stale text.
Fix: the UpdateView handler's state write for the view record read by GetView/ListViews.
Traces: cloudwatchomni-view-update-and-conflict-errors#task#5.

## Evidence
Cause 1 - cloudwatchomni-access-profile-and-grant-lifecycle#task#6 GetAccessProfile, expected 200 / got 200: $.accessProfile.description expected 'second', got 'first'; $.accessProfile.assumeStatus expected 'ALLOWED', got 'DENIED'.
Cause 2 - cloudwatchomni-omni-dashboard-update-and-validation#task#7 GetOmniDashboard, expected 200 / got 200: $.omniDashboard.description expected 'second', got 'first'.
Cause 3 - cloudwatchomni-view-update-and-conflict-errors#task#5 GetView, expected 200 / got 200: $.description expected 'second', got 'first'.
('first' and 'second' are the fixtures' request-supplied description values; the requirement is that the read returns the value last written, not these literals.)

## Steps to Reproduce
Replay simparity-wire-aws-v1. In each fixture, run the Create step, then the Update step that sets a new description, then the Get step: cloudwatchomni-access-profile-and-grant-lifecycle#task#6, cloudwatchomni-omni-dashboard-update-and-validation#task#7, cloudwatchomni-view-update-and-conflict-errors#task#5. Each Get answers 200 carrying the pre-update description.
