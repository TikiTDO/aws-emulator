Issue-Source: trace-sweep-divergence
Issue-Trace: sso-admin-describe-list-edge-cases-r2
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sso-admin-describe-list-edge-cases-r2
Issue-Shape-Id: com.amazonaws.ssoadmin#SWBExternalService
Issue-Key: sso-admin|ListInstances|2xx->2xx|$.Instances[Name="sim2real-wire-idc-faa37ffa"].State
Issue-Commit: 1fca6b81b3c9f5fd973c0414004e0261bbfba86a

# ListInstances drops the State member from every serialized Identity Center instance

> _Note: this issue was identified against the code base at commit `1fca6b81b3c9f5fd973c0414004e0261bbfba86a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstances

## Description
ONE cause.

Cause 1 of 1: ListInstances serializes each listed Identity Center instance without the State view the live API serves (fixture sso-admin-describe-list-edge-cases-r2, step task#4).

AWS: ListInstances returns Instances, a list of InstanceMetadata. Smithy: ListInstancesResponse.Instances -> InstanceList (@length min 0 max 10) of com.amazonaws.ssoadmin#InstanceMetadata, whose modelled members are InstanceArn, IdentityStoreId, IdentityStoreArn, OwnerAccountId, Name, CreatedDate, Status (InstanceStatus enum), StatusReason, PrimaryRegion and Regions; errors AccessDeniedException/InternalServerException/ThrottlingException/ValidationException. The on-disk model declares NO State member on InstanceMetadata, so for State this is recorded behavior only: the recorded 200 carries, on EVERY listed instance, an additional State structure whose single member Name holds that instance's lifecycle value, emitted alongside the flat Status member and agreeing with it (an active instance lists Status "ACTIVE" and State {"Name": "ACTIVE"}; an instance still being enabled would list CREATE_IN_PROGRESS in both). State.Name is read from the listed instance's own stored status - it is not fixed text. The instance's identity values in any one recording (InstanceArn, Name, IdentityStoreId/IdentityStoreArn, CreatedDate) are per-run values of the recording environment and are NOT requirements.

Emulator: HTTP 200 whose listed instance matches on every other member but carries no State member at all - $.Instances[Name="<generated instance name>"].State expected {'Name': 'ACTIVE'}, got None. The flat Status member is emitted correctly, so the stored lifecycle is right and the omission is purely in the shared InstanceMetadata serialization. No status code and no error code diverged.

Why: a client that reads an instance's lifecycle from the State view the current API serves - filtering a listing for State.Name == "ACTIVE", or polling ListInstances until a newly enabled instance leaves CREATE_IN_PROGRESS - dereferences a member absent on every entry. It either faults on the missing key or treats every instance as being in an unknown state, and can never distinguish an active instance from one still being created.

Fix: the InstanceMetadata response serializer shared by ListInstances - emit State from the same stored instance status the flat Status member is written from, so every listed entry carries State.Name.

Related open issues, NONE of which covers this cluster: 20261029T184546Z-sso-admin-instance-reads-drop-the-state-member-and-the.md, 20261030T004149Z-sso-admin-listinstances-omits-state-on-the-instance-enumerated-by.md, 20261030T043338Z-sso-admin-enumeration-responses-omit-output-members-listapplicationproviders-returns-no.md, 20261030T084159Z-sso-admin-listinstances-omits-the-state-member-on-listed-identity.md, 20261030T113738Z-sso-admin-listinstances-omits-state-on-each-listed-identity-center.md and 20261030T123631Z-sso-admin-listinstances-entry-for-the-probe-created-instance-omits.md all describe this same serializer omission, but each pins its recorded replay key to a JSONPath selector containing the instance NAME that its own replay happened to generate ("sim2real-wire-idc-c56c0824", "sim2real-wire-idc-04f3bea9", "IdentityCenter", ...). This fixture's instance name is generated fresh on every run (this replay enumerated "sim2real-wire-idc-faa37ffa"), so none of those keys joins this cluster and the replay report reports it unowned. Filed so this cluster is actually re-replayed before the defect is closed. A fixer should note the real requirement is name-independent: EVERY entry of Instances must carry State, so fixing the one serializer site closes this and all the related issues at once.

Traces: sso-admin-describe-list-edge-cases-r2#task#4.

## Evidence
Fixture sso-admin-describe-list-edge-cases-r2, step task#4, operation ListInstances, service sso-admin, expected HTTP 200 / actual HTTP 200 (verdict: mismatch).

  $.Instances[Name="sim2real-wire-idc-faa37ffa"].State: missing - expected {'Name': 'ACTIVE'}, got None

The instance name inside the JSONPath selector is a per-run generated value and is not part of the requirement. The requirement is that every entry of Instances carries a State structure whose Name equals that instance's status - here ACTIVE, matching the flat Status member the emulator did emit correctly. No other member of the entry diverged; no HTTP status, error code or error message is involved in this divergence.

Smithy evidence that State is recorded behavior only: com.amazonaws.ssoadmin#InstanceMetadata members are InstanceArn, IdentityStoreId, IdentityStoreArn, OwnerAccountId, Name, CreatedDate, Status, StatusReason, PrimaryRegion, Regions - no State member is modelled, so the recorded response is the authority for its presence and content. ListInstancesResponse itself models only Instances and NextToken.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from this checkout.
2. Replay the sso-admin wire fixtures of trace set simparity-wire-aws-v1.
3. Replay fixture sso-admin-describe-list-edge-cases-r2 up to and including step task#4 (earlier steps enable an Identity Center instance whose name is generated per run, then task#4 calls ListInstances).
4. Inspect the 200 body of task#4: the entry for the generated instance carries Status "ACTIVE" but has no State member.
5. Expected: that entry (and every other entry of Instances) also carries State: {"Name": "ACTIVE"}, a State structure whose Name mirrors that instance's stored status.
