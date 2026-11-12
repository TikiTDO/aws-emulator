Issue-Source: trace-sweep-divergence
Issue-Trace: sso-admin-describe-list-edge-cases-r2
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sso-admin-describe-list-edge-cases-r2
Issue-Shape-Id: com.amazonaws.ssoadmin#SWBExternalService
Issue-Key: sso-admin|ListInstances|2xx->2xx|$.Instances[Name="sim2real-wire-idc-aab42d7b"].State
Issue-Commit: 2aeffd5e5ca9a10c283c8a29444e9b7abf34c8f4

# ListInstances entry for the probe-created instance omits the State member

> _Note: this issue was identified against the code base at commit `2aeffd5e5ca9a10c283c8a29444e9b7abf34c8f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstances

## Description
ONE cause: the ListInstances response serializer never emits the per-instance State view for an instance created during the fixture's own describe/list edge-case setup.

Cause 1 of 1: ListInstances omits State on a probe-created Identity Center instance (fixture sso-admin-describe-list-edge-cases-r2, step task#4).
AWS: each member of ListInstances' Instances list carries, in addition to the modeled flat members (Smithy, com.amazonaws.ssoadmin#InstanceMetadata: InstanceArn, IdentityStoreId, IdentityStoreArn, OwnerAccountId, Name, CreatedDate, Status, StatusReason, PrimaryRegion, Regions; ListInstances output Instances -> InstanceList @length max 10, NextToken -> Token), a `State` structure whose `Name` is the instance's lifecycle status - {"Name": "ACTIVE"} beside Status "ACTIVE". The on-disk model has NO State member on InstanceMetadata, so this is recorded behavior only: the recording is the authority. State.Name mirrors whatever status the listed instance holds in the emulator's state (ACTIVE for an active instance, CREATE_IN_PROGRESS/CREATE_FAILED otherwise); it is derived from state, never a constant. The instance's identity values in any one recording (its generated Name, InstanceArn, IdentityStoreId/IdentityStoreArn, CreatedDate) are per-run values and are NOT part of the requirement.
Emulator: 200 whose listed instance matches on every other member but has no State member at all - $.Instances[Name="sim2real-wire-idc-<suffix>"].State expected {'Name': 'ACTIVE'}, got None. No status, no error code, no message diverges; only the member is absent.
Why: a client that filters a listing on State.Name, or polls ListInstances waiting for an instance it just created to reach ACTIVE, dereferences a missing member on every instance and can never observe the transition - it cannot distinguish an ACTIVE instance from one still being created.
Fix: the shared InstanceMetadata response serializer used by ListInstances - populate State from the same instance record that already supplies Status, so every listed instance carries State.Name.
Traces: sso-admin-describe-list-edge-cases-r2#task#4.

Relation to an already-open issue: 20261029T184546Z-sso-admin-instance-reads-drop-the-state-member-and-the.md describes the same omission class on DescribeInstance and on three other ListInstances clusters, and its Cause 2 is this same fixture and step. It does NOT cover this cluster: its recorded key pins the instance name 'sim2real-wire-idc-c56c0824', while this replay's instance is generated as 'sim2real-wire-idc-aab42d7b', so the cluster key does not join and the replay report leaves it unowned (owned_by empty). The generated instance name is a per-run value, not a requirement - the behavioral requirement is identical. A fixer closing that issue will very likely close this one with the same edit, but this cluster carries its own replay key and must be re-replayed to confirm.

## Evidence
Fixture sso-admin-describe-list-edge-cases-r2, step task#4, operation ListInstances, expected HTTP 200 / actual HTTP 200 (status matches; body member missing):
  $.Instances[Name="sim2real-wire-idc-aab42d7b"].State: missing - expected {'Name': 'ACTIVE'}, got None

The instance name in that JSONPath is a per-run generated value; the requirement is that EVERY member of Instances carries a State structure whose Name equals that instance's Status. The rest of the listed member matched the recording exactly, including Status: 'ACTIVE' - so the emulator holds the status and simply does not serialize the State view.

Model evidence (services/service-com-amazonaws-ssoadmin-swbexternalservice/smithy-model/com-amazonaws-ssoadmin-swbexternalservice.json): InstanceMetadata declares InstanceArn, IdentityStoreId, IdentityStoreArn, OwnerAccountId, Name, CreatedDate, Status, StatusReason, PrimaryRegion, Regions and no State member - hence "recorded behavior only" for State.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the sso-admin wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture sso-admin-describe-list-edge-cases-r2 through step task#4: the fixture's earlier steps create an Identity Center instance (name generated as sim2real-wire-idc-<suffix>), then task#4 calls ListInstances.
3. Inspect the 200 body: the created instance appears in $.Instances with Status 'ACTIVE' but no 'State' member.
4. Expected: that member also carries State: {"Name": "ACTIVE"} - State.Name equal to the instance's Status. Actual: State absent.
