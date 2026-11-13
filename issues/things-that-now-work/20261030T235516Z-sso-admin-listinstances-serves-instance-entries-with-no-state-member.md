Issue-Source: trace-sweep-divergence
Issue-Trace: sso-admin-describe-list-edge-cases-r2
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sso-admin-describe-list-edge-cases-r2
Issue-Shape-Id: com.amazonaws.ssoadmin#SWBExternalService
Issue-Key: sso-admin|ListInstances|2xx->2xx|$.Instances[Name="sim2real-wire-idc-d6ee869b"].State
Issue-Commit: ddba316ee385da4fbf562967d92dafa29ff99585

# ListInstances serves instance entries with no State member in the describe/list edge-case sequence

> _Note: this issue was identified against the code base at commit `ddba316ee385da4fbf562967d92dafa29ff99585`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstances

## Description
ONE cause.

Cause 1 of 1: ListInstances omits the per-instance State structure (fixture sso-admin-describe-list-edge-cases-r2, step task#4).

AWS: ListInstances returns Instances as an InstanceList of InstanceMetadata members (Smithy: ListInstancesResponse.Instances -> InstanceList @length min 0 max 10; ListInstances errors AccessDeniedException 403, InternalServerException 500, ThrottlingException 429, ValidationException 400). The on-disk model's InstanceMetadata stops at InstanceArn, IdentityStoreId, IdentityStoreArn, OwnerAccountId, Name, CreatedDate, Status, StatusReason, PrimaryRegion, Regions - it declares no State member, so for State this is recorded behavior only: the recording shows each listed Identity Center instance additionally carrying a State structure whose single member Name mirrors the instance's lifecycle status enum value, served alongside the flat Status member (State {"Name": "ACTIVE"} beside Status "ACTIVE" for an active instance). State.Name is a value read from the instance's own state, not a constant; the instance's Name, ARNs, identity-store id and CreatedDate in any single recording are per-run values and are not requirements.

Emulator: HTTP 200 whose listed instance matches on every other member but has no State member at all: $.Instances[Name="sim2real-wire-idc-d6ee869b"].State expected {'Name': 'ACTIVE'}, got None. (The instance name in that path is the per-run name the fixture created; the defect is the absent member, not the name.)

Why: a client that reads an instance's lifecycle from the State view the current API serves - filtering a listing on State.Name, or polling ListInstances until State.Name becomes ACTIVE - dereferences a missing member on every instance and can never distinguish ACTIVE from CREATE_IN_PROGRESS or CREATE_FAILED. No status or error code diverges here; the response is a well-formed 200 with an incomplete instance shape.

Fix: the shared InstanceMetadata response serializer used by ListInstances - populate State from the same instance record that already supplies Status, so State.Name carries the instance's status enum value.

Related open issue, which does NOT cover this cluster: 20261029T184546Z-sso-admin-instance-reads-drop-the-state-member-and-the.md describes the same omission in the instance serializer, but its recorded replay keys bind to a different instance identity on this fixture/step (it names $.Instances[Name="sim2real-wire-idc-c56c0824"].State), so this cluster is not owned by it and is reported unowned by the replay. A fixer closing that issue should re-replay this step too; a single serializer change is expected to satisfy both.

Traces: sso-admin-describe-list-edge-cases-r2#task#4.

## Evidence
Fixture sso-admin-describe-list-edge-cases-r2, step task#4, operation ListInstances, expected HTTP 200 / actual HTTP 200 (verdict: mismatch).

Diverging key:
  $.Instances[Name="sim2real-wire-idc-d6ee869b"].State: missing - expected {'Name': 'ACTIVE'}, got None

No other key on the step diverged: the listed instance's InstanceArn, IdentityStoreId, IdentityStoreArn, OwnerAccountId, Name, CreatedDate, Status, PrimaryRegion and Regions all matched. The instance name 'sim2real-wire-idc-d6ee869b' and the identity-store/instance ARNs are per-run values from the fixture's own setup and are excluded from the requirement; the requirement is that each InstanceMetadata member carries State with Name equal to that instance's status enum value ('ACTIVE' for an active instance).

Divergence key reported by the replay: service=sso-admin|ListInstances|status=200->200|keys=$.Instances[Name="sim2real-wire-idc-d6ee869b"].State

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the sso-admin wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture sso-admin-describe-list-edge-cases-r2 through step task#4, which calls ListInstances after the fixture's describe/list edge-case setup has created and renamed its instance.
3. Inspect the 200 response body: each entry of $.Instances has Status 'ACTIVE' but no State member.
4. Expected: every InstanceMetadata entry also carries State: {"Name": "ACTIVE"} (State.Name mirroring that instance's status enum value).
