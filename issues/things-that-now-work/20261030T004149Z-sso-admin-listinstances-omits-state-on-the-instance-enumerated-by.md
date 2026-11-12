Issue-Source: trace-sweep-divergence
Issue-Trace: sso-admin-describe-list-edge-cases-r2
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sso-admin-describe-list-edge-cases-r2
Issue-Shape-Id: com.amazonaws.ssoadmin#SWBExternalService
Issue-Key: sso-admin|ListInstances|2xx->2xx|$.Instances[Name="sim2real-wire-idc-04f3bea9"].State
Issue-Commit: 716594ad03747236be5f362ba3ba8170b7a2d152

# ListInstances omits State on the instance enumerated by the describe/list edge-case fixture

> _Note: this issue was identified against the code base at commit `716594ad03747236be5f362ba3ba8170b7a2d152`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstances

## Description
ONE cause.

Cause 1 of 1: ListInstances serves instance members without the State view (fixture sso-admin-describe-list-edge-cases-r2, step task#4).

AWS: ListInstances returns Instances, a list of InstanceMetadata. Smithy: ListInstancesResponse.Instances -> InstanceList (@length max 10) of com.amazonaws.ssoadmin#InstanceMetadata, whose modelled members are InstanceArn, IdentityStoreId, IdentityStoreArn, OwnerAccountId, Name, CreatedDate, Status (InstanceStatus enum), StatusReason, PrimaryRegion and Regions. The on-disk model declares NO State member, so for State this is recorded behavior only: the recorded 200 carries, on each listed instance, an additional State structure whose single member Name holds that instance's lifecycle value, served alongside the flat Status member and agreeing with it (an active instance lists Status "ACTIVE" and State {"Name": "ACTIVE"}). State.Name is read from the listed instance's own stored status, not fixed text; the instance's identity values in any one recording (InstanceArn, Name, IdentityStoreId/IdentityStoreArn, CreatedDate) are per-run values of the environment that recorded it and are not requirements.

Emulator: 200 whose listed instance matches on every other member but carries no State member at all - $.Instances[Name="<instance name>"].State expected {'Name': 'ACTIVE'}, got None. Status itself is emitted correctly; only the State structure is dropped, so the omission is in the shared InstanceMetadata serialization rather than in the instance's stored lifecycle.

Why: a client that reads an instance's lifecycle from the State view that the current API serves - filtering a listing for State.Name == "ACTIVE", or polling ListInstances until a newly enabled instance leaves CREATE_IN_PROGRESS - dereferences a member that is absent on every entry. It therefore either crashes on the missing key or treats every instance as being in an unknown state, and cannot distinguish an active instance from one still being created.

Fix: the InstanceMetadata response serializer shared by ListInstances - emit State from the same stored instance status the flat Status member is written from, so every listed entry carries State.Name.

Related open issue, which does NOT cover this cluster: 20261029T184546Z-sso-admin-instance-reads-drop-the-state-member-and-the.md describes the same omission and names this fixture and step in its prose, but its recorded replay keys pin the ListInstances divergence to instances named "sim2real-wire-idc-c56c0824" and "IdentityCenter". The instance name in this fixture is generated per run (this replay enumerated "sim2real-wire-idc-04f3bea9"), so that issue's key does not join this cluster and the replay report reports it unowned. Filed so this cluster is actually re-replayed before the defect is closed; a fixer closing both should fix the single serializer site once.

Traces: sso-admin-describe-list-edge-cases-r2#task#4.

## Evidence
Fixture sso-admin-describe-list-edge-cases-r2, step task#4, operation ListInstances, expected HTTP 200 / actual HTTP 200 (verdict: mismatch).

  $.Instances[Name="sim2real-wire-idc-04f3bea9"].State: missing - expected {'Name': 'ACTIVE'}, got None

The instance name in the JSONPath selector is a per-run generated value; the requirement is that EVERY entry of Instances carries a State structure whose Name equals that instance's status (here ACTIVE, matching the Status member the emulator did emit). No other member of the entry diverged, and no status or error-code divergence is involved.

Smithy evidence that this is recorded behavior only: com.amazonaws.ssoadmin#InstanceMetadata members are InstanceArn, IdentityStoreId, IdentityStoreArn, OwnerAccountId, Name, CreatedDate, Status, StatusReason, PrimaryRegion, Regions - no State member is modelled, so the recorded response is the authority for its presence and content.

## Steps to Reproduce
1. Clone the repo and boot the emulator from this checkout.
2. Replay the sso-admin wire corpus of trace set simparity-wire-aws-v1.
3. Replay fixture sso-admin-describe-list-edge-cases-r2 up to and including step task#4 (the setup steps enable an Identity Center instance whose name is generated per run, then task#4 calls ListInstances).
4. Inspect the 200 body of task#4: the entry for the generated instance carries Status "ACTIVE" but no State member.
5. Expected: that entry also carries State: {"Name": "ACTIVE"}, i.e. a State structure whose Name mirrors the instance's status.
