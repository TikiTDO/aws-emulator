Issue-Source: trace-sweep-divergence
Issue-Trace: pca-connector-ad-directory-registration-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pca-connector-ad-directory-registration-lifecycle
Issue-Shape-Id: com.amazonaws.pcaconnectorad#PcaConnectorAd
Issue-Key: pca-connector-ad|ListDirectoryRegistrations|2xx->2xx|$.DirectoryRegistrations[*].Status
Issue-Key: pca-connector-ad|ListDirectoryRegistrations|2xx->2xx|$.DirectoryRegistrations[Arn="arn
Issue-Key: pca-connector-ad|GetDirectoryRegistration|2xx->2xx|$.DirectoryRegistration.Status
Issue-Key: pca-connector-ad|GetDirectoryRegistration|2xx->2xx|$.DirectoryRegistration.StatusReason
Issue-Key: pca-connector-ad|GetDirectoryRegistration|4xx->2xx|$
Issue-Key: pca-connector-ad|GetDirectoryRegistration|4xx->2xx|$.Message
Issue-Key: pca-connector-ad|GetDirectoryRegistration|4xx->2xx|$.ResourceId
Issue-Key: pca-connector-ad|GetDirectoryRegistration|4xx->2xx|$.ResourceType
Issue-Key: pca-connector-ad|GetDirectoryRegistration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: pca-connector-ad|GetDirectoryRegistration|4xx->2xx|status
Issue-Commit: fd57799079cf971de3edcb569520acb45a060d60

# DirectoryRegistration lifecycle: a registration for a nonexistent directory never reaches FAILED, and a deleted one keeps answering 200

> _Note: this issue was identified against the code base at commit `fd57799079cf971de3edcb569520acb45a060d60`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDirectoryRegistration

## Description
Three causes, all in the pca-connector-ad DirectoryRegistration lifecycle: the emulator settles every registration to ACTIVE no matter whether the addressed Active Directory exists, and it never removes a registration that was deleted. Service model: services/service-com-amazonaws-pcaconnectorad-pcaconnectorad/smithy-model/com-amazonaws-pcaconnectorad-pcaconnectorad.json. Open sibling issues cover different ground and do NOT cover any of these: 20261029T224504Z-pca-connector-ad-createconnector-skips-the-directoryregistration-prerequisite-and-persists-a.md (CreateConnector not checking that a registration exists), 20261029T190840Z-pca-connector-ad-diverges-on-seven-counts-validationexception.md (validation message rendering, CreateConnector authz, CreateTemplate Name pattern, not-found wording, UntagResource stub). A fixer must close all THREE causes below and re-replay all three steps named in them.

=== Cause 1 of 3: ListDirectoryRegistrations reports Status=ACTIVE with no StatusReason for a registration whose directory does not exist, instead of FAILED / DIRECTORY_RESOURCE_NOT_FOUND ===
AWS: CreateDirectoryRegistration is asynchronous (HTTP 202) and does not verify the directory inline; the registration is created in CREATING and the service then attempts to authorize AWS Private CA with the named Active Directory. When that directory does not exist in Directory Service, the registration terminates in Status FAILED with StatusReason DIRECTORY_RESOURCE_NOT_FOUND, and every reporting operation shows that terminal state. Smithy: DirectoryRegistrationStatus enum = [CREATING, ACTIVE, DELETING, FAILED]; DirectoryRegistrationStatusReason enum = [DIRECTORY_ACCESS_DENIED, DIRECTORY_RESOURCE_NOT_FOUND, DIRECTORY_NOT_ACTIVE, DIRECTORY_NOT_REACHABLE, DIRECTORY_TYPE_NOT_SUPPORTED, INTERNAL_FAILURE]; DirectoryRegistration.StatusReason is documented "Additional information about the directory registration status if the status is failed"; ListDirectoryRegistrations is GET /directoryRegistrations, 200, items DirectoryRegistrations. Which reason code an unknown directory id yields (DIRECTORY_RESOURCE_NOT_FOUND) is recorded behavior only — the model enumerates the value but does not bind it to a condition.
Emulator: the registration settles unconditionally from CREATING to ACTIVE and StatusReason is never populated. ListDirectoryRegistrations returned the registration with Status 'ACTIVE' and no StatusReason, and the fixture's poll for FAILED gave up after 58 attempts still seeing 'ACTIVE'.
Why: a client that waits for the registration to become usable (poll until ACTIVE, or until FAILED and then surface StatusReason) never learns that the directory it named does not exist. On AWS such a workflow fails fast with a diagnosable reason code; against the emulator it reports success and the caller goes on to CreateConnector against a directory that cannot be authorized, so the misconfiguration surfaces much later or not at all. The FAILED branch of any lifecycle error handling is dead code against the emulator.
Fix: the clock-owed state transition in the service's state store plus the create path that seeds it — services/service-com-amazonaws-pcaconnectorad-pcaconnectorad/src/state.rs (DirectoryRegistrationRecord.active_at and the `settle` routine, which only knows CREATING -> ACTIVE; the module defines only DIRECTORY_REGISTRATION_STATUS_CREATING and DIRECTORY_REGISTRATION_STATUS_ACTIVE and no FAILED constant) and services/.../src/ops/create_directory_registration.rs (the record it inserts always schedules an ACTIVE settle). A registration whose DirectoryId is not a directory the emulator knows must settle to FAILED with StatusReason DIRECTORY_RESOURCE_NOT_FOUND instead.
Traces: pca-connector-ad-directory-registration-lifecycle#task#2.

=== Cause 2 of 3: GetDirectoryRegistration reports the same phantom ACTIVE state for that registration ===
AWS: GetDirectoryRegistration (GET /directoryRegistrations/{DirectoryRegistrationArn}, 200) returns the SAME terminal state as the list call for the registration of a nonexistent directory: DirectoryRegistration.Status = FAILED and DirectoryRegistration.StatusReason = DIRECTORY_RESOURCE_NOT_FOUND. Smithy: GetDirectoryRegistration output member DirectoryRegistration -> DirectoryRegistration structure carrying Status/StatusReason as above; status 200. The reason code binding is recorded behavior only.
Emulator: returns 200 with DirectoryRegistration.Status = 'ACTIVE' and StatusReason absent. The read path itself is faithful — services/.../src/ops/get_directory_registration.rs:160-194 calls `store.settle(...)` and then copies record.status / record.status_reason straight onto the wire shape — so it faithfully reports whatever Cause 1 wrote.
Why: the single-resource read is the call an SDK waiter uses, so a caller polling GetDirectoryRegistration for a terminal state is told the registration is healthy; it also means the emulator never exercises a client's StatusReason handling. This cluster is a separate replay unit from Cause 1 (different operation and step) and must be re-run on its own even though it shares the root cause.
Fix: same component as Cause 1 — the FAILED settle in src/state.rs seeded by src/ops/create_directory_registration.rs; get_directory_registration.rs needs no change beyond reporting the corrected record.
Traces: pca-connector-ad-directory-registration-lifecycle#task#3.

=== Cause 3 of 3: after DeleteDirectoryRegistration the registration is still served, so GetDirectoryRegistration answers 200 forever instead of 404 ResourceNotFoundException ===
AWS: DeleteDirectoryRegistration (DELETE /directoryRegistrations/{DirectoryRegistrationArn}, 202, @idempotent) deauthorizes AWS Private CA with the directory and removes the registration; the resource passes through DELETING and then stops existing. A subsequent GetDirectoryRegistration on that ARN is refused with HTTP 404, error code ResourceNotFoundException (x-amzn-errortype: ResourceNotFoundException) and a body carrying all three required members: Message = `DirectoryRegistration was not found: <DirectoryRegistrationArn exactly as supplied in the request>.` (fixed text `DirectoryRegistration was not found: ` and the trailing period; the variable part is the request's ARN), ResourceId = that same ARN from the request, ResourceType = `DirectoryRegistration`. Smithy: GetDirectoryRegistration declares ResourceNotFoundException @httpError 404 ("The operation tried to access a nonexistent resource..."); ResourceNotFoundException requires Message, ResourceId and ResourceType, and ResourceType is documented as one of Connector, Template, TemplateGroupAccessControlEntry, ServicePrincipalName, DirectoryRegistration. The exact message sentence is recorded behavior only.
Emulator: the registration is never removed, so GetDirectoryRegistration keeps answering HTTP 200 with the full DirectoryRegistration body (Arn, DirectoryId, Status 'ACTIVE', CreatedAt, UpdatedAt) and no error members at all; the fixture polled for the 404 and still saw 200 after 29 attempts. Root cause: services/service-com-amazonaws-pcaconnectorad-pcaconnectorad/src/ops/delete_directory_registration.rs is still a scaffold — the file carries the `SCAFFOLDED, NOT IMPLEMENTED` marker and its `handle` returns `crate::scaffolded::Scaffolded::not_implemented("DeleteDirectoryRegistration")` at line 191 — so nothing ever mutates the store. Note that the not-found refusal itself is already correctly worded in get_directory_registration.rs:170-182, so once the delete removes the record this step yields the required status, code and message.
Why: a deleted registration that keeps reading back as ACTIVE breaks teardown and idempotency logic in every client: cleanup appears to succeed yet the resource is still there, re-creating the same directory registration is refused as a conflict, and any test that asserts the resource is gone passes on AWS and hangs or fails against the emulator. State accumulates for the life of the process.
Fix: implement services/.../src/ops/delete_directory_registration.rs — remove (or move to DELETING and then remove) the addressed DirectoryRegistrationRecord in src/state.rs, answer the modeled 202, stay idempotent for an ARN that is already gone, and delete the SCAFFOLDED marker line.
Traces: pca-connector-ad-directory-registration-lifecycle#teardown#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up=true, /__admin/health 200). Fixture pca-connector-ad-directory-registration-lifecycle: registration created for directory id d-9067fedc21, which is not an existing directory; ARN arn:aws:pca-connector-ad:us-west-2:000000000000:directory-registration/d-9067fedc21 (the account/region/id values are this recording's, not part of the requirement).

Cause 1 — pca-connector-ad-directory-registration-lifecycle#task#2, ListDirectoryRegistrations, status 200 expected and 200 received, body diverges:
  $.DirectoryRegistrations[Arn="...directory-registration/d-9067fedc21"].Status: expected 'FAILED', got 'ACTIVE'
  $.DirectoryRegistrations[Arn="...directory-registration/d-9067fedc21"].StatusReason: expected 'DIRECTORY_RESOURCE_NOT_FOUND', got missing
  $.DirectoryRegistrations[0].Status: poll: not in ['FAILED'] after 58 attempt(s) (still 'ACTIVE')

Cause 2 — pca-connector-ad-directory-registration-lifecycle#task#3, GetDirectoryRegistration, status 200 both sides:
  $.DirectoryRegistration.Status: expected 'FAILED', got 'ACTIVE'
  $.DirectoryRegistration.StatusReason: expected 'DIRECTORY_RESOURCE_NOT_FOUND', got missing

Cause 3 — pca-connector-ad-directory-registration-lifecycle#teardown#5, GetDirectoryRegistration after DeleteDirectoryRegistration:
  status: expected 404, got 200
  $headers.x-amzn-errortype: expected ResourceNotFoundException (shaped), got none
  $.Message: expected 'DirectoryRegistration was not found: arn:aws:pca-connector-ad:us-west-2:000000000000:directory-registration/d-9067fedc21.', got missing
  $.ResourceId: expected the same ARN, got missing
  $.ResourceType: expected 'DirectoryRegistration', got missing
  $: poll: status 200 not in [404] after 29 attempt(s); body still {'DirectoryRegistration': {'Arn': '...directory-registration/d-9067fedc21', 'DirectoryId': 'd-9067fedc21', 'Status': 'ACTIVE', 'CreatedAt': <ts>, 'UpdatedAt': <ts>}}

Code: src/state.rs (DIRECTORY_REGISTRATION_STATUS_CREATING/ACTIVE only, no FAILED; DirectoryRegistrationRecord.active_at drives an unconditional CREATING -> ACTIVE settle), src/ops/create_directory_registration.rs:240-258 (always schedules the ACTIVE settle, status_reason: None), src/ops/get_directory_registration.rs:160-194 (settles then mirrors the record), src/ops/delete_directory_registration.rs:4 and :191 (SCAFFOLDED marker; handle returns not_implemented).

## Steps to Reproduce
Boot the emulator and point an AWS client at it (the recording used region us-west-2).

Cause 1 and 2:
1. CreateDirectoryRegistration(DirectoryId="d-9067fedc21") — a pattern-valid directory id that is not an existing Active Directory. It returns 202 with DirectoryRegistrationArn arn:aws:pca-connector-ad:<region>:<account>:directory-registration/d-9067fedc21.
2. Poll ListDirectoryRegistrations() until the entry for that ARN reaches a terminal status. AWS reaches Status=FAILED with StatusReason=DIRECTORY_RESOURCE_NOT_FOUND; the emulator reaches Status=ACTIVE with no StatusReason and never leaves it (58 polls).
3. Call GetDirectoryRegistration(DirectoryRegistrationArn=<that ARN>). AWS returns DirectoryRegistration.Status=FAILED and StatusReason=DIRECTORY_RESOURCE_NOT_FOUND; the emulator returns Status=ACTIVE and omits StatusReason.

Cause 3:
4. Call DeleteDirectoryRegistration(DirectoryRegistrationArn=<that ARN>) (AWS: 202).
5. Poll GetDirectoryRegistration(DirectoryRegistrationArn=<that ARN>). AWS eventually refuses with HTTP 404, x-amzn-errortype ResourceNotFoundException and the body {Message: "DirectoryRegistration was not found: <that ARN>.", ResourceId: "<that ARN>", ResourceType: "DirectoryRegistration"}. The emulator keeps returning HTTP 200 with the registration still present and Status ACTIVE (29 polls), because DeleteDirectoryRegistration is an unimplemented scaffold and removes nothing.

Replay the three recorded steps directly: pca-connector-ad-directory-registration-lifecycle#task#2, #task#3 and #teardown#5.
