Issue-Source: trace-sweep-divergence
Issue-Trace: iot-managed-integrations-credential-locker-lifecycle
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-managed-integrations-credential-locker-lifecycle iot-managed-integrations-device-discovery-listing-filters iot-managed-integrations-event-log-configuration-scoped-lifecycle iot-managed-integrations-managed-thing-lifecycle iot-managed-integrations-ota-task-configuration-lifecycle
Issue-Shape-Id: com.amazonaws.iotmanagedintegrations#IotManagedIntegrations
Issue-Key: iot-managed-integrations|GetCredentialLocker|2xx->2xx|$.Id
Issue-Key: iot-managed-integrations|ListCredentialLockers|2xx->2xx|$.Items[Arn="arn:aws:iotmanagedintegrations:eu-west-1:000000000000:credential-locker/18e6cd141522f84f00000000"].Id
Issue-Key: iot-managed-integrations|GetDeviceDiscovery|2xx->2xx|$.ControllerId
Issue-Key: iot-managed-integrations|GetDeviceDiscovery|2xx->2xx|$.Id
Issue-Key: iot-managed-integrations|GetEventLogConfiguration|2xx->2xx|$.Id
Issue-Key: iot-managed-integrations|GetEventLogConfiguration|2xx->2xx|$.ResourceId
Issue-Key: iot-managed-integrations|ListEventLogConfigurations|2xx->2xx|$.EventLogConfigurationList[*]
Issue-Key: iot-managed-integrations|GetManagedThing|2xx->2xx|$.Id
Issue-Key: iot-managed-integrations|ListManagedThings|2xx->2xx|$.Items[Arn="arn:aws:iotmanagedintegrations:eu-west-1:000000000000:managed-thing/18e6cd12208cbbd600000000"].Id
Issue-Key: iot-managed-integrations|GetOtaTaskConfiguration|2xx->2xx|$.TaskConfigurationId
Issue-Commit: 14df30a096a54aecb31fd12ed18f88f92038b3f7

# Read and list responses diverge on the echoed resource identifier members

> _Note: this issue was identified against the code base at commit `14df30a096a54aecb31fd12ed18f88f92038b3f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCredentialLocker

## Description
Eight read/list steps of iot-managed-integrations answer the modelled status and the modelled body shape, yet the replay reports each one diverging on exactly the members that carry a resource identifier (Id, ResourceId, ControllerId, TaskConfigurationId, or the identifier inside a list element). These are the read side of the same identifier mint reported in the companion issue titled "Create responses return clock-derived opaque identifiers the corpus's identifier variables reject": every value involved was produced by `crate::state::opaque_id` (services/service-com-amazonaws-iotmanagedintegrations-iotmanagedintegrations/src/state.rs), which is 16 hex digits of SystemTime::now() nanoseconds plus an 8-hex-digit store counter. In these clusters the expected and actual identifiers RENDER identically in the replay detail, so the difference is not in the visible text of the member but in the identifier's provenance/format. Hypothesis (neither the model nor the recording states it): the corpus pins these members to an identifier variable whose binding the emulator's clock+counter identifier does not satisfy, so each echo of that identifier is reported again on the step that serves it. This issue bundles EIGHT causes; all eight must be closed.

Cause 1 of 8: GetCredentialLocker (fixture iot-managed-integrations-credential-locker-lifecycle, step task#3).
AWS: Smithy: GetCredentialLocker -> GET /credential-lockers/{Identifier}, success 200; output Id (CredentialLockerId, @length 1..64, @pattern ^[a-zA-Z0-9]*$) must be the identifier of the locker addressed by the path label.
Emulator: 200, body shape right, $.Id reported diverging from the pinned locker identifier (expected and actual render the same 24-char hex value).
Why: a client reading back a locker sees the identifier the create minted, so it inherits the emulator-shaped value everywhere it correlates lockers.
Fix: the projection of the stored locker onto the output shape in src/ops/get_credential_locker.rs, together with the mint in src/state.rs `opaque_id`.
Traces: iot-managed-integrations-credential-locker-lifecycle#task#3.

Cause 2 of 8: ListCredentialLockers (fixture iot-managed-integrations-credential-locker-lifecycle, step task#4).
AWS: Smithy: ListCredentialLockers -> GET /credential-lockers, 200, paginated, output Items (CredentialLockerListDefinition) whose entries carry the locker's Id and Arn.
Emulator: 200; the element selected by its Arn matches on the Arn but its .Id is reported diverging - i.e. within one element the ARN's resource segment is accepted while the Id member is not.
Why: a client listing lockers and keying results by Items[].Id keys on a value the corpus does not accept, while the same identifier inside the ARN is accepted - an inconsistency no AWS response has.
Fix: the list projection in src/ops/list_credential_lockers.rs (the Id member it copies out of the stored record) and the mint in src/state.rs.
Traces: iot-managed-integrations-credential-locker-lifecycle#task#4.

Cause 3 of 8: GetDeviceDiscovery (fixture iot-managed-integrations-device-discovery-listing-filters, step task#3).
AWS: Smithy: GetDeviceDiscovery -> GET /device-discoveries/{Identifier}, 200; output Id (DeviceDiscoveryId, required, @length 1..200, @pattern ^[A-Za-z0-9]+$) and ControllerId (ManagedThingId, @length 1..64, @pattern ^[a-zA-Z0-9:_-]*$) - the discovery's own identifier and the managed thing that performed it.
Emulator: 200 with BOTH $.Id and $.ControllerId reported diverging; both values come from the same mint.
Why: discovery results are correlated by discovery id and by controller id; both handles a client would persist are emulator-shaped.
Fix: src/ops/get_device_discovery.rs (its Id/ControllerId projection) and the mint in src/state.rs.
Traces: iot-managed-integrations-device-discovery-listing-filters#task#3.

Cause 4 of 8: GetEventLogConfiguration (fixture iot-managed-integrations-event-log-configuration-scoped-lifecycle, step task#3).
AWS: Smithy: GetEventLogConfiguration -> GET /event-log-configurations/{Id}, 200; output Id (LogConfigurationId, @length 1..200, @pattern ^[A-Za-z0-9]+$) and ResourceId (SmartHomeResourceId, @length 1..200, @pattern ^[a-zA-Z0-9+*]*$) - the configuration's identifier and the identifier of the resource it is scoped to.
Emulator: 200 with $.Id and $.ResourceId both reported diverging; both are mint-produced ids (the configuration's own, and the credential locker it scopes).
Why: the scoping link between a log configuration and its target resource is read back through these two members; both carry emulator-shaped identifiers.
Fix: src/ops/get_event_log_configuration.rs and the mint in src/state.rs.
Traces: iot-managed-integrations-event-log-configuration-scoped-lifecycle#task#3.

Cause 5 of 8: ListEventLogConfigurations (fixture iot-managed-integrations-event-log-configuration-scoped-lifecycle, step task#4).
AWS: Smithy: ListEventLogConfigurations -> GET /event-log-configurations, 200, paginated, output EventLogConfigurationList; each entry carries Id, ResourceType, ResourceId and EventLogLevel.
Emulator: 200, but the expected first list element is reported MISSING rather than mismatched: the element the emulator serves carries the same four members (ResourceType 'credential-locker', the DEBUG level, and the configuration/resource ids) yet does not match the expected element, which is identified by its Id.
Why: a client paging log configurations cannot find the configuration it just created by matching on the element the corpus pins - a list whose entries do not match the resources they describe breaks every read-after-write flow built on this list.
Fix: the list projection in src/ops/list_event_log_configurations.rs (the element it emits for a stored configuration) and the mint in src/state.rs.
Traces: iot-managed-integrations-event-log-configuration-scoped-lifecycle#task#4.

Cause 6 of 8: GetManagedThing (fixture iot-managed-integrations-managed-thing-lifecycle, step task#2; the same cluster also covers step task#5).
AWS: Smithy: GetManagedThing -> GET /managed-things/{Identifier}, 200; output Id (ManagedThingId, @length 1..64, @pattern ^[a-zA-Z0-9:_-]*$) is the identifier of the thing addressed by the path label.
Emulator: 200 with $.Id reported diverging, on the read before the update (task#2) and on the read after it (task#5).
Why: the managed thing's identifier is the handle for every subsequent call (update, delete, discovery, log configuration); a client reads back an emulator-shaped value both before and after mutation.
Fix: src/ops/get_managed_thing.rs and the mint in src/state.rs.
Traces: iot-managed-integrations-managed-thing-lifecycle#task#2, iot-managed-integrations-managed-thing-lifecycle#task#5.

Cause 7 of 8: ListManagedThings (fixture iot-managed-integrations-managed-thing-lifecycle, step task#3).
AWS: Smithy: ListManagedThings -> GET /managed-things, 200, paginated, output Items (ManagedThingListDefinition) whose entries carry Id and Arn.
Emulator: 200; the element selected by its managed-thing Arn matches on the Arn while its .Id is reported diverging - the same intra-element inconsistency as Cause 2.
Why: clients enumerate things and then address them by Items[].Id; that member must be the same accepted identifier the ARN embeds.
Fix: the list projection in src/ops/list_managed_things.rs and the mint in src/state.rs.
Traces: iot-managed-integrations-managed-thing-lifecycle#task#3.

Cause 8 of 8: GetOtaTaskConfiguration (fixture iot-managed-integrations-ota-task-configuration-lifecycle, step task#4).
AWS: Smithy: GetOtaTaskConfiguration -> GET /ota-task-configurations/{Identifier}, 200; output TaskConfigurationId (OtaTaskConfigurationId, @length 1..64, @pattern ^[a-zA-Z0-9]*$).
Emulator: 200 with $.TaskConfigurationId reported diverging from the pinned configuration identifier.
Why: the OTA configuration handle read back is the emulator-shaped value, so clients storing it store a value AWS would never mint.
Fix: src/ops/get_ota_task_configuration.rs and the mint in src/state.rs.
Traces: iot-managed-integrations-ota-task-configuration-lifecycle#task#4.

No open issue covers these read/list identifier echoes. The status and shape divergences already filed for this service (DeleteCloudConnector 404, DeleteOtaTaskConfiguration wording, UpdateManagedThing Content-Length, the custom-endpoint address) are different operations or different members and do not carry these replay keys.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up). Each cluster matched the expected status and diverged only on identifier-bearing members:

Cause 1 - GetCredentialLocker, iot-managed-integrations-credential-locker-lifecycle#task#3, 200/200: "$.Id: value_mismatch (expected '18e6cd141522f84f00000000', got '18e6cd141522f84f00000000')" - expected and actual render identically.

Cause 2 - ListCredentialLockers, iot-managed-integrations-credential-locker-lifecycle#task#4, 200/200: "$.Items[Arn=\"arn:aws:iotmanagedintegrations:eu-west-1:000000000000:credential-locker/18e6cd141522f84f00000000\"].Id: value_mismatch (expected '18e6cd141522f84f00000000', got '18e6cd141522f84f00000000')" - the element's Arn matched (it served as the selector) while its Id did not.

Cause 3 - GetDeviceDiscovery, iot-managed-integrations-device-discovery-listing-filters#task#3, 200/200: "$.ControllerId: value_mismatch (expected '18e6cd1286708ede00000000', got '18e6cd1286708ede00000000')" and "$.Id: value_mismatch (expected '18e6cd1286de18ac00000001', got '18e6cd1286de18ac00000001')".

Cause 4 - GetEventLogConfiguration, iot-managed-integrations-event-log-configuration-scoped-lifecycle#task#3, 200/200: "$.Id: value_mismatch (expected '18e6cd14aa8ff0b500000001', ...)" and "$.ResourceId: value_mismatch (expected '18e6cd14a9a62c4a00000000', ...)".

Cause 5 - ListEventLogConfigurations, iot-managed-integrations-event-log-configuration-scoped-lifecycle#task#4, 200/200: "$.EventLogConfigurationList[0]: missing (expected {'EventLogLevel': 'DEBUG', 'Id': '18e6cd14aa8ff0b500000001', 'ResourceId': '18e6cd14a9a62c4a00000000', 'ResourceType': 'credential-locker'}, got {'Id': '18e6cd14aa8ff0b500000001', 'ResourceType': 'credential-locker', 'ResourceId': '18e6cd14a9a62c4a00000000', 'EventLogLevel': 'DEBUG'})" - verdict 'missing', i.e. the expected element was not matched in the served list.

Cause 6 - GetManagedThing, iot-managed-integrations-managed-thing-lifecycle#task#2 (cluster also covers task#5), 200/200: "$.Id: value_mismatch (expected '18e6cd12208cbbd600000000', got '18e6cd12208cbbd600000000')".

Cause 7 - ListManagedThings, iot-managed-integrations-managed-thing-lifecycle#task#3, 200/200: "$.Items[Arn=\"arn:aws:iotmanagedintegrations:eu-west-1:000000000000:managed-thing/18e6cd12208cbbd600000000\"].Id: value_mismatch (expected '18e6cd12208cbbd600000000', got '18e6cd12208cbbd600000000')".

Cause 8 - GetOtaTaskConfiguration, iot-managed-integrations-ota-task-configuration-lifecycle#task#4, 200/200: "$.TaskConfigurationId: value_mismatch (expected '18e6cd14777e9e6b00000000', got '18e6cd14777e9e6b00000000')".

Every value named above has the structure src/state.rs `opaque_id` emits: format!("{:016x}{counter:08x}", SystemTime::now() nanos as u64).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay iot-managed-integrations-credential-locker-lifecycle through task#3 (GET /credential-lockers/{Identifier}) and compare $.Id.
3. Cause 2: continue to task#4 (GET /credential-lockers) and compare Items[].Id against the element's own Arn resource segment.
4. Cause 3: replay iot-managed-integrations-device-discovery-listing-filters task#3 (GET /device-discoveries/{Identifier}) and compare $.Id and $.ControllerId.
5. Cause 4: replay iot-managed-integrations-event-log-configuration-scoped-lifecycle task#3 (GET /event-log-configurations/{Id}) and compare $.Id and $.ResourceId.
6. Cause 5: continue to task#4 (GET /event-log-configurations) and check whether the expected first element is matched at all.
7. Cause 6: replay iot-managed-integrations-managed-thing-lifecycle task#2 and task#5 (GET /managed-things/{Identifier}) and compare $.Id.
8. Cause 7: replay the same fixture's task#3 (GET /managed-things) and compare Items[].Id.
9. Cause 8: replay iot-managed-integrations-ota-task-configuration-lifecycle task#4 (GET /ota-task-configurations/{Identifier}) and compare $.TaskConfigurationId.
10. Each step returns the modelled status and body shape; only the identifier-bearing members are reported diverging, and they change on every replay because the mint embeds the wall clock.
