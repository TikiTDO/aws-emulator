Issue-Source: trace-sweep-divergence
Issue-Trace: sso-admin-describe-list-edge-cases-r2
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sso-admin-describe-list-edge-cases-r2
Issue-Shape-Id: com.amazonaws.ssoadmin#SWBExternalService
Issue-Key: sso-admin|ListInstances|2xx->2xx|$.Instances[Name="sim2real-wire-idc-382be23f"].State
Issue-Commit: 65d3d2622d8873a86aa4906c5e08ba578fc14a3d

# InstanceMetadata serialization omits the State view, so the edge-case ListInstances entry carries no lifecycle state

> _Note: this issue was identified against the code base at commit `65d3d2622d8873a86aa4906c5e08ba578fc14a3d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstances

## Description
ONE cause.

Cause 1 of 1: ListInstances serializes each listed Identity Center instance without the State view, so the entry enumerated by the describe/list edge-case fixture has no lifecycle state (fixture sso-admin-describe-list-edge-cases-r2, step task#4).

AWS: ListInstances returns Instances, a list of InstanceMetadata. Smithy (com-amazonaws-ssoadmin-swbexternalservice.json): ListInstancesResponse.Instances -> com.amazonaws.ssoadmin#InstanceList (@length min 0, max 10) of com.amazonaws.ssoadmin#InstanceMetadata, whose modelled members are InstanceArn, IdentityStoreId, IdentityStoreArn, OwnerAccountId, Name, CreatedDate, Status (InstanceStatus enum), StatusReason, PrimaryRegion and Regions. The on-disk model declares NO State member, so for State this is recorded behavior only: the recorded HTTP 200 carries, on each listed instance, an additional State structure whose single member Name holds that instance's lifecycle value, served alongside the flat Status member and agreeing with it (an instance whose Status is ACTIVE also lists State {"Name": "ACTIVE"}). State.Name is read from the listed instance's own stored status; it is not fixed text. No status code or error code is involved - both sides are 200 - so there is no error message requirement here. The instance's identity values in any one recording (InstanceArn, Name, IdentityStoreId/IdentityStoreArn, CreatedDate) are per-run values of the environment that recorded it and are not requirements.

Emulator: HTTP 200 whose listed instance matches on every other member, including the flat Status, but carries no State member at all - $.Instances[Name="<generated instance name>"].State expected {'Name': 'ACTIVE'}, got None. Because Status itself is emitted correctly, the omission is in the shared InstanceMetadata response serialization, not in the instance's stored lifecycle.

Why: a client that reads an instance's lifecycle from the State view the current API serves - filtering a listing on State.Name == "ACTIVE", or polling ListInstances until a newly enabled instance leaves CREATE_IN_PROGRESS - dereferences a key that is absent on every entry. It either fails on the missing member or treats every instance as being in an unknown state, and cannot tell an active instance from one still being created.

Fix: the InstanceMetadata response serializer shared by ListInstances - emit State from the same stored instance status the flat Status member is written from, so every listed entry carries State.Name.

Related open issues, which do NOT cover this cluster: 20261029T184546Z-sso-admin-instance-reads-drop-the-state-member-and-the.md and 20261030T004149Z-sso-admin-listinstances-omits-state-on-the-instance-enumerated-by.md describe the same omission, and the latter even names this fixture and step; but their recorded replay keys pin the divergence to instance names generated in earlier runs (e.g. "sim2real-wire-idc-c56c0824", "sim2real-wire-idc-04f3bea9", "IdentityCenter"), while this replay enumerated "sim2real-wire-idc-382be23f". The per-run instance name is part of the JSONPath selector, so those keys do not join this cluster and the replay report reports it unowned. Filed so this cluster is actually re-replayed before the defect is closed; a fixer closing it should fix the single serializer site once, which closes the siblings too.

Traces: sso-admin-describe-list-edge-cases-r2#task#4.

## Evidence
Fixture sso-admin-describe-list-edge-cases-r2, step task#4, operation ListInstances, service sso-admin, expected HTTP 200 / actual HTTP 200 (verdict: mismatch), divergence key service=sso-admin|ListInstances|status=200->200|keys=$.Instances[Name="sim2real-wire-idc-382be23f"].State, owned_by: (empty), stub_path: (empty).

  $.Instances[Name="sim2real-wire-idc-382be23f"].State: missing - expected {'Name': 'ACTIVE'}, got None

The instance name inside the JSONPath selector is a per-run generated value, not a requirement. The requirement is that EVERY entry of Instances carries a State structure whose Name equals that instance's status (here ACTIVE, which matches the flat Status member the emulator did emit). No other member of the entry diverged; no status-code and no error-code/error-message divergence is involved.

Smithy evidence that this is recorded behavior only: com.amazonaws.ssoadmin#InstanceMetadata members are InstanceArn, IdentityStoreId, IdentityStoreArn, OwnerAccountId, Name, CreatedDate, Status, StatusReason, PrimaryRegion, Regions - no State member is modelled, so the recorded response is the authority for its presence and content.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from this checkout.
2. Replay the wire corpus of trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Replay fixture sso-admin-describe-list-edge-cases-r2 through step task#4: the preceding steps enable an Identity Center instance whose name is generated per run, then task#4 calls ListInstances.
4. Inspect the 200 body of task#4: the entry for the generated instance carries Status "ACTIVE" but no State member.
5. Expected: that entry also carries State: {"Name": "ACTIVE"} - a State structure whose Name mirrors the instance's stored status - on every listed instance.
