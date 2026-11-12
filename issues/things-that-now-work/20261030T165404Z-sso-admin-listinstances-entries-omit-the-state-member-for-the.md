Issue-Source: trace-sweep-divergence
Issue-Trace: sso-admin-describe-list-edge-cases-r2
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sso-admin-describe-list-edge-cases-r2
Issue-Shape-Id: com.amazonaws.ssoadmin#SWBExternalService
Issue-Key: sso-admin|ListInstances|2xx->2xx|$.Instances[Name="sim2real-wire-idc-819ad5bf"].State
Issue-Commit: 0deae0340a86101143c629cc9b774419f34d5675

# ListInstances entries omit the State member for the edge-case instance listing

> _Note: this issue was identified against the code base at commit `0deae0340a86101143c629cc9b774419f34d5675`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstances

## Description
ONE cause. A fixer must close it before submitting.

Cause 1 of 1: ListInstances serializes each listed Identity Center instance without the `State` member (fixture sso-admin-describe-list-edge-cases-r2, step task#4).

AWS: recorded behavior only. Each member of ListInstances' `Instances` list carries, in addition to the modelled InstanceMetadata members, a `State` structure whose single member `Name` holds the instance's lifecycle value, mirroring the instance's `Status` enum value — so an instance served with Status "ACTIVE" is served with State {"Name": "ACTIVE"}. The on-disk Smithy model is explicit that it does not describe this: com.amazonaws.ssoadmin#InstanceMetadata declares only InstanceArn, IdentityStoreId, IdentityStoreArn, OwnerAccountId, Name, CreatedDate, Status, StatusReason, PrimaryRegion and Regions, and com.amazonaws.ssoadmin#ListInstancesResponse declares only Instances (InstanceList, @length max 10) and NextToken — there is no State member on either. The recording is therefore the authority for this member. The response is a 200 with no error code or message, so no error wording is at issue. The instance's identity values in the matched member (InstanceArn, Name, IdentityStoreId/IdentityStoreArn, OwnerAccountId, CreatedDate, Regions[].AddedDate) are read from the emulator's own instance record and are per-run values, not requirements; the requirement is the presence of State and that State.Name equals the same instance's Status.

Emulator: answers 200 and matches the listed instance on every other member, but the member has no State at all — $.Instances[Name="sim2real-wire-idc-819ad5bf"].State: expected {'Name': 'ACTIVE'}, got None (the instance name in that path is a per-run generated value from this fixture's setup, not a requirement).

Why: a client that reads an instance's lifecycle from State.Name while enumerating instances — the shape the current API serves — dereferences a missing member on every listed instance and cannot distinguish ACTIVE from CREATE_IN_PROGRESS/CREATE_FAILED, so a wait-for-active or filter-by-state loop over a listing either crashes or never settles.

Fix: the shared InstanceMetadata response serializer used by ListInstances (the same site that builds each listed instance member) — emit State from the instance record's status alongside Status.

Traces: sso-admin-describe-list-edge-cases-r2#task#4.

Relation to open issues: 20261029T184546Z-sso-admin-instance-reads-drop-the-state-member-and-the.md describes the same family of omission, but its replay keys bind to a different ListInstances cluster of this fixture (the instance named sim2real-wire-idc-c56c0824) and to the other two fixtures; the replay report does not report this cluster as owned by it, so this recorded cluster is not covered and must be re-replayed on its own. 20261029T112447Z-sso-admin-listinstances-pagination.md concerns NextToken decoding on the same operation, not the serialized instance shape.

## Evidence
Fixture sso-admin-describe-list-edge-cases-r2, step task#4, operation ListInstances.
Expected status 200 / actual status 200 (no status or error-code divergence; a missing output member).
Verdict: mismatch.
Diff reported by the replay:
  $.Instances[Name="sim2real-wire-idc-819ad5bf"].State: missing (expected {'Name': 'ACTIVE'}, got None)
Every other member of that listed instance matched. The instance name embedded in the JSONPath selector is a per-run generated value; the requirement evidenced here is that each listed instance carries State and that State.Name equals that instance's Status ("ACTIVE" here).
Model evidence (Smithy, services/service-com-amazonaws-ssoadmin-swbexternalservice/smithy-model/com-amazonaws-ssoadmin-swbexternalservice.json): InstanceMetadata members = InstanceArn, IdentityStoreId, IdentityStoreArn, OwnerAccountId, Name, CreatedDate, Status, StatusReason, PrimaryRegion, Regions — no State; ListInstancesResponse members = Instances (InstanceList, @length 0..10), NextToken (@length 0..2048, pattern ^[-a-zA-Z0-9+=/_]*$). Hence "recorded behavior only" for State.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the sso-admin wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture sso-admin-describe-list-edge-cases-r2 through step task#4, which calls ListInstances after the fixture's describe/list edge-case setup has created its instance.
3. Inspect the 200 response body: each entry of $.Instances carries InstanceArn, IdentityStoreId, IdentityStoreArn, OwnerAccountId, Name, CreatedDate, Status, PrimaryRegion and Regions, but no State member.
4. Expected: the entry also carries State as a structure, State.Name equal to that entry's Status (ACTIVE for an active instance).
