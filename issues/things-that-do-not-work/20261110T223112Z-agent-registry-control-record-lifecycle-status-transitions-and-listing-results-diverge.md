Issue-Source: trace-sweep-divergence
Issue-Trace: agent-registry-control-edits-return-pending-and-rejected-records-to-draft
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: agent-registry-control-edits-return-pending-and-rejected-records-to-draft agent-registry-control-record-token-replay-rename-and-descriptor-errors
Issue-Shape-Id: com.amazonaws.agentregistrycontrol#AgentRegistryControl
Issue-Key: agent-registry-control|GetRegistryRecord|2xx->2xx|$.description
Issue-Key: agent-registry-control|SubmitRegistryRecordForApproval|2xx->4xx|$.recordArn
Issue-Key: agent-registry-control|SubmitRegistryRecordForApproval|2xx->4xx|$.recordId
Issue-Key: agent-registry-control|SubmitRegistryRecordForApproval|2xx->4xx|$.registryArn
Issue-Key: agent-registry-control|SubmitRegistryRecordForApproval|2xx->4xx|$.status
Issue-Key: agent-registry-control|SubmitRegistryRecordForApproval|2xx->4xx|$.updatedAt
Issue-Key: agent-registry-control|SubmitRegistryRecordForApproval|2xx->4xx|status
Issue-Key: agent-registry-control|CreateRegistryRecord|2xx->2xx|$.status
Issue-Key: agent-registry-control|ListRegistryRecords|2xx->2xx|$.registryRecords[*]
Issue-Related: 20261104T045821Z-agent-registry-control-agentregistrycontrol-record-reads-ignore-every-mutation-after-create.md
Issue-Related: 20261104T045821Z-agent-registry-control-agentregistrycontrol-serializes-date-time-timestamps-as-epoch-numbers.md
Issue-Commit: 677ec28d2ee9442908109a9e8e0002b68ccccfcc

# Record lifecycle status transitions and listing results diverge from AWS

> _Note: this issue was identified against the code base at commit `677ec28d2ee9442908109a9e8e0002b68ccccfcc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubmitRegistryRecordForApproval

## Description
The emulator's registry-record state machine does not match AWS: the status a record is created in is wrong, an edit does not move a record out of the approval workflow, the follow-up submit is therefore refused, and the record listing returns a record AWS's listing does not. This bundle has FOUR causes; all four must be fixed and re-replayed.

Cause 1 of 4 — an edit to a record in the approval workflow does not return it to DRAFT (fixture agent-registry-control-edits-return-pending-and-rejected-records-to-draft, step task#9).
AWS: recorded behavior only for the transition rule — the model documents UpdateRegistryRecord as "Updates a registry record. The update is asynchronous: the record is returned with the UPDATING status while it is processed. Fields that use update wrappers follow PATCH semantics", and SubmitRegistryRecordForApproval as "Submits a DRAFT registry record for approval"; RegistryRecordStatus is an enum of DRAFT, PENDING_APPROVAL, APPROVED, REJECTED, DEPRECATED, CREATING, UPDATING, CREATE_FAILED, UPDATE_FAILED. The recording shows that updating a record that is in PENDING_APPROVAL (or REJECTED) takes it back out of the approval workflow: GetRegistryRecord settles at status DRAFT and reflects the edited description (its own @length 1..4096, @sensitive member).
Emulator: GetRegistryRecord returns 200 with status PENDING_APPROVAL and no description member; the fixture's poll for DRAFT never settles (not in ['DRAFT'] after 40 attempts).
Why: a client that edits a pending record and waits for it to return to DRAFT hangs forever, and anything keyed on the record's status after an edit reads a stale workflow state. The missing description overlaps the already-open issue 20261104T045821Z-agent-registry-control-agentregistrycontrol-record-reads-ignore-every-mutation-after-create.md; the status-reset rule reported here is NOT covered by it.
Fix: UpdateRegistryRecord's handler/state transition — an update applied to a PENDING_APPROVAL or REJECTED record must reset the record's status to DRAFT (via UPDATING) as well as apply the PATCH-semantics field changes.
Traces: agent-registry-control-edits-return-pending-and-rejected-records-to-draft#task#9 (also #task#15).

Cause 2 of 4 — SubmitRegistryRecordForApproval refuses with 409 the re-submit AWS accepts with 202 (fixture agent-registry-control-edits-return-pending-and-rejected-records-to-draft, step task#16).
AWS: Smithy: SubmitRegistryRecordForApproval POST /registries/{registryId}/records/{recordId}/submit-for-approval, success code 202, output members registryArn, recordArn, recordId, status (required, RegistryRecordStatus) and updatedAt (required, @timestampFormat date-time) — all five required. A record that an edit returned to DRAFT can be submitted again and the response carries status PENDING_APPROVAL (per the operation's doc: "Depending on the registry's approval configuration, the record is either auto-approved or set to PENDING_APPROVAL").
Emulator: HTTP 409 ConflictException with none of the five members present.
Why: the approval workflow cannot be re-driven after an edit — a curate/edit/resubmit loop dead-ends at a conflict the real service never raises. This follows from Cause 1 (the record is still PENDING_APPROVAL in emulator state), but the step's own requirement is the 202 submit response.
Fix: SubmitRegistryRecordForApproval's status precondition in its handler, together with the Cause-1 transition in record state.
Traces: agent-registry-control-edits-return-pending-and-rejected-records-to-draft#task#16.

Cause 3 of 4 — CreateRegistryRecord returns status DRAFT instead of CREATING (fixture agent-registry-control-record-token-replay-rename-and-descriptor-errors, step task#7).
AWS: Smithy: CreateRegistryRecord, success code 202, output member status required (RegistryRecordStatus); operation doc: "Creation is asynchronous: the record is returned with the CREATING status while it is processed." So the create response's status is CREATING, and the record reaches DRAFT only once creation completes.
Emulator: 202 with status DRAFT in the create response — while the same emulator's ListRegistryRecords reports a freshly created record as CREATING, so the two surfaces disagree with each other as well as with AWS.
Why: a client that waits for CREATING to clear before using the record never sees the transition, and one that asserts the documented create status fails against the emulator.
Fix: CreateRegistryRecord's handler — set the new record's initial status to CREATING and report it in the create response consistently with the record state the read/list paths serve.
Traces: agent-registry-control-record-token-replay-rename-and-descriptor-errors#task#7.

Cause 4 of 4 — ListRegistryRecords returns a record that is not in AWS's listing (fixture agent-registry-control-record-token-replay-rename-and-descriptor-errors, step task#13).
AWS: Smithy: ListRegistryRecords POST /registries/{registryId}/records-list, 200, output registryRecords required (list of RegistryRecordSummary), paginated (nextToken/maxResults, items registryRecords); input member filters -> list of RegistryRecordFilter { name (enum: name, status, recordType), values (exactly 1 value) }, up to 10 filters. The listing returns only the records matching the request's filters — in the recording this listing has no entry at registryRecords[0], i.e. no record of the registry matches.
Emulator: returns one RegistryRecordSummary at index 0 (a CUSTOM record of the registry, reported with status CREATING).
Why: a client that lists by name, status or recordType is handed records it explicitly filtered out, so it acts on resources outside its selection. Hypothesis: the handler ignores the request's filters member and returns every record of the registry.
Fix: ListRegistryRecords' handler — evaluate each RegistryRecordFilter (name/status/recordType) against record state before building the page, and apply it before pagination.
Traces: agent-registry-control-record-token-replay-rename-and-descriptor-errors#task#13.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (emulator up, 112 steps, 37 mismatched):

Cause 1 — GetRegistryRecord, fixture agent-registry-control-edits-return-pending-and-rejected-records-to-draft step task#9: $.status expected 'DRAFT', got 'PENDING_APPROVAL' ($.status poll: not in ['DRAFT'] after 40 attempts); $.description missing (expected 'Storm surge alerts', got None). Cluster also covers #task#15.

Cause 2 — SubmitRegistryRecordForApproval, step task#16: status expected 202, got 409; $.recordArn, $.recordId, $.registryArn, $.status (expected 'PENDING_APPROVAL') and $.updatedAt all missing from the response.

Cause 3 — CreateRegistryRecord, fixture agent-registry-control-record-token-replay-rename-and-descriptor-errors step task#7: status 202 as expected but $.status expected 'CREATING', got 'DRAFT'.

Cause 4 — ListRegistryRecords, fixture agent-registry-control-record-token-replay-rename-and-descriptor-errors step task#13: $.registryRecords[0] extra — expected no entry, got a RegistryRecordSummary for a CUSTOM record of the registry with status 'CREATING'.

(The createdAt/updatedAt epoch-number mismatches reported in neighbouring steps belong to the already-open timestamp-encoding issue, and the clusters marked owned_by the existing SubmitRegistryRecordForApproval and record-reads issues are excluded here.)

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
3. Fixture agent-registry-control-edits-return-pending-and-rejected-records-to-draft:
   - create a registry and a record, SubmitRegistryRecordForApproval so the record is PENDING_APPROVAL, then UpdateRegistryRecord to change its description.
   - step task#9: GetRegistryRecord and poll for status DRAFT -> emulator stays PENDING_APPROVAL for 40 attempts and omits the new description.
   - step task#16: SubmitRegistryRecordForApproval on that edited record -> expect 202 with registryArn/recordArn/recordId/status PENDING_APPROVAL/updatedAt, emulator returns 409 ConflictException.
4. Fixture agent-registry-control-record-token-replay-rename-and-descriptor-errors:
   - step task#7: CreateRegistryRecord -> expect the 202 body's status to be CREATING, emulator returns DRAFT.
   - step task#13: ListRegistryRecords with filters on the registry -> expect an empty registryRecords list, emulator returns a non-matching CUSTOM record.
