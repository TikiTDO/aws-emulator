Issue-Source: trace-sweep-divergence
Issue-Trace: iot-managed-integrations-credential-locker-lifecycle
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-managed-integrations-credential-locker-lifecycle iot-managed-integrations-event-log-configuration-scoped-lifecycle iot-managed-integrations-event-log-configuration-wildcard-lifecycle iot-managed-integrations-managed-thing-lifecycle iot-managed-integrations-ota-task-configuration-lifecycle
Issue-Shape-Id: com.amazonaws.iotmanagedintegrations#IotManagedIntegrations
Issue-Key: iot-managed-integrations|CreateCredentialLocker|2xx->2xx|$.Arn
Issue-Key: iot-managed-integrations|CreateCredentialLocker|2xx->2xx|$.Id
Issue-Key: iot-managed-integrations|CreateEventLogConfiguration|2xx->2xx|$.Id
Issue-Key: iot-managed-integrations|CreateManagedThing|2xx->2xx|$.Id
Issue-Key: iot-managed-integrations|CreateOtaTaskConfiguration|2xx->2xx|$.TaskConfigurationId
Issue-Commit: 14df30a096a54aecb31fd12ed18f88f92038b3f7

# Create responses return clock-derived opaque identifiers the corpus's identifier variables reject

> _Note: this issue was identified against the code base at commit `14df30a096a54aecb31fd12ed18f88f92038b3f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCredentialLocker

## Description
Five create operations of iot-managed-integrations answer the right status but a resource identifier the wire corpus does not accept. Every one of them mints its identifier through the SAME helper, `crate::state::opaque_id` (services/service-com-amazonaws-iotmanagedintegrations-iotmanagedintegrations/src/state.rs, `pub fn opaque_id(counter: u64)`), which formats the wall clock in nanoseconds as 16 lowercase hex digits followed by an 8-hex-digit store counter: a 24-character value whose leading digits are identical for every resource created in the same few seconds and whose trailing digits are a visible, zero-padded creation counter. This issue bundles FIVE causes; all five must be closed.

Hypothesis (not stated by the model or the recording): the corpus pins each create response's identifier to a per-fixture identifier variable, and the emulator's clock-plus-counter identifier does not satisfy that binding, so the pinned member is reported as a mismatch while the status matches. The replay reports the diverging members per step; the shared component is the identifier mint, not any one operation's state keeping.

Cause 1 of 5: CreateCredentialLocker (fixture iot-managed-integrations-credential-locker-lifecycle, step task#1).
AWS: Smithy: CreateCredentialLocker -> POST /credential-lockers, success 201; output Id (CredentialLockerId, @length 1..64, @pattern ^[a-zA-Z0-9]*$) and Arn (CredentialLockerArn, @length 32..1011, @pattern ^arn:aws:iotmanagedintegrations:[0-9a-zA-Z-]+:[0-9]+:credential-locker/[0-9a-zA-Z]+$). AWS mints one opaque locker identifier and echoes it as the ARN's resource segment. The recorded step pins Id to the fixture's locker-identifier variable and Arn to 'arn:aws:iotmanagedintegrations:<region>:<account>:credential-locker/<that same variable>'; the fixed text of the ARN is the arn prefix, the service namespace 'iotmanagedintegrations' and the resource type 'credential-locker', and its variable parts are region, account id and the minted locker id.
Emulator: 201 with Id and Arn both diverging on the pinned value; the Id served is a 24-char lowercase-hex clock+counter value and the ARN carries it as its resource segment.
Why: a client that stores the identifier a create returns, or that parses the ARN's resource segment, gets a value whose shape is a property of this emulator's clock rather than of the service, so identifier handling validated against the emulator (fixed width, uniqueness characteristics, ordering that leaks creation time) is not what AWS exercises.
Fix: the identifier mint used by the CreateCredentialLocker handler - `opaque_id` in src/state.rs and its call site in src/ops/create_credential_locker.rs.
Traces: iot-managed-integrations-credential-locker-lifecycle#task#1.

Cause 2 of 5: CreateCredentialLocker in the event-log fixture's setup (fixture iot-managed-integrations-event-log-configuration-scoped-lifecycle, step setup#1).
AWS: same modelled contract as Cause 1 (POST /credential-lockers, 201, Id = CredentialLockerId). The step pins Id to this fixture's own scoped-locker variable, which later steps reuse as the event-log configuration's ResourceId.
Emulator: 201 with $.Id diverging from the pinned variable; served value is another clock+counter id.
Why: the locker this fixture's whole scoped-logging flow hangs off is created under an identifier the corpus does not accept, so every scoped read downstream is anchored on an emulator-shaped id.
Fix: same mint and handler as Cause 1.
Traces: iot-managed-integrations-event-log-configuration-scoped-lifecycle#setup#1.

Cause 3 of 5: CreateEventLogConfiguration (fixture iot-managed-integrations-event-log-configuration-scoped-lifecycle, step setup#2; the same cluster also covers iot-managed-integrations-event-log-configuration-wildcard-lifecycle#task#1).
AWS: Smithy: CreateEventLogConfiguration -> POST /event-log-configurations, success 201; output Id (LogConfigurationId, @length 1..200, @pattern ^[A-Za-z0-9]+$). The step pins Id to the fixture's event-log-configuration variable.
Emulator: 201 with $.Id diverging; served value is a clock+counter id from the same helper.
Why: a client correlating a log configuration by the id the create returned - including the wildcard flow, which reaches the same cluster - keys on an emulator-shaped identifier.
Fix: the identifier mint used by src/ops/create_event_log_configuration.rs (`opaque_id` in src/state.rs).
Traces: iot-managed-integrations-event-log-configuration-scoped-lifecycle#setup#2, iot-managed-integrations-event-log-configuration-wildcard-lifecycle#task#1.

Cause 4 of 5: CreateManagedThing (fixture iot-managed-integrations-managed-thing-lifecycle, step setup#1).
AWS: Smithy: CreateManagedThing -> POST /managed-things, success 201; output Id (ManagedThingId, @length 1..64, @pattern ^[a-zA-Z0-9:_-]*$) and Arn (ManagedThingArn, @pattern ^arn:aws:iotmanagedintegrations:[0-9a-zA-Z-]+:[0-9]+:managed-thing/([0-9a-zA-Z:_-])+$). The step pins Id to the fixture's managed-thing variable.
Emulator: 201 with $.Id diverging; served value is a clock+counter id.
Why: the managed thing is the central resource of this service; its identifier is what clients persist, filter on (ListManagedThings ParentControllerIdentifierFilter, CredentialLockerFilter) and embed in ARNs.
Fix: the identifier mint used by src/ops/create_managed_thing.rs (`opaque_id` in src/state.rs).
Traces: iot-managed-integrations-managed-thing-lifecycle#setup#1.

Cause 5 of 5: CreateOtaTaskConfiguration (fixture iot-managed-integrations-ota-task-configuration-lifecycle, step setup#1).
AWS: Smithy: CreateOtaTaskConfiguration -> POST /ota-task-configurations, success 201; output TaskConfigurationId (OtaTaskConfigurationId, @length 1..64, @pattern ^[a-zA-Z0-9]*$). The step pins TaskConfigurationId to the fixture's ota-task-configuration variable.
Emulator: 201 with $.TaskConfigurationId diverging; served value is a clock+counter id.
Why: the OTA task configuration id is the handle every later OTA call takes; a client holding it holds an emulator-shaped value.
Fix: the identifier mint used by src/ops/create_ota_task_configuration.rs (`opaque_id` in src/state.rs).
Traces: iot-managed-integrations-ota-task-configuration-lifecycle#setup#1.

No open issue covers identifier minting for this service; the nearest neighbours (the custom-endpoint address issue 20261109T113716Z-iot-managed-integrations-custom-endpoint-announces-an-unreachable.md and the CreateEventLogConfiguration conflict-message issue 20261109T165826Z-...) are about an announced host and an error message, not about the identifiers a create returns.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200):

Cause 1 - CreateCredentialLocker, iot-managed-integrations-credential-locker-lifecycle#task#1, expected status 201 / actual 201: "$.Arn: value_mismatch (expected 'arn:aws:iotmanagedintegrations:eu-west-1:000000000000:credential-locker/{{locker_id}}', got 'arn:aws:iotmanagedintegrations:eu-west-1:000000000000:credential-locker/18e6cd141522f84f00000000')" and "$.Id: value_mismatch (expected '{{locker_id}}', got '18e6cd141522f84f00000000')".

Cause 2 - CreateCredentialLocker, iot-managed-integrations-event-log-configuration-scoped-lifecycle#setup#1, 201/201: "$.Id: value_mismatch (expected '{{scoped_locker_id}}', got '18e6cd14a9a62c4a00000000')".

Cause 3 - CreateEventLogConfiguration, iot-managed-integrations-event-log-configuration-scoped-lifecycle#setup#2, 201/201: "$.Id: value_mismatch (expected '{{scoped_event_log_config_id}}', got '18e6cd14aa8ff0b500000001')"; cluster also affects iot-managed-integrations-event-log-configuration-wildcard-lifecycle#task#1.

Cause 4 - CreateManagedThing, iot-managed-integrations-managed-thing-lifecycle#setup#1, 201/201: "$.Id: value_mismatch (expected '{{managed_thing_id}}', got '18e6cd12208cbbd600000000')".

Cause 5 - CreateOtaTaskConfiguration, iot-managed-integrations-ota-task-configuration-lifecycle#setup#1, 201/201: "$.TaskConfigurationId: value_mismatch (expected '{{ota_task_configuration_id}}', got '18e6cd14777e9e6b00000000')".

The served values share the structure the mint produces: 16 hex digits of SystemTime::now() nanoseconds (all of them begin '18e6cd1', the corpus having replayed inside one ~10-second window) followed by an 8-hex-digit sequence ('00000000', '00000001'). Source: src/state.rs `opaque_id` -> format!("{:016x}{counter:08x}", nanos as u64).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: POST /credential-lockers (fixture iot-managed-integrations-credential-locker-lifecycle, step task#1) and compare $.Id and $.Arn with the step's pinned locker identifier.
3. Cause 2: replay iot-managed-integrations-event-log-configuration-scoped-lifecycle step setup#1 and compare $.Id.
4. Cause 3: replay iot-managed-integrations-event-log-configuration-scoped-lifecycle step setup#2 (and iot-managed-integrations-event-log-configuration-wildcard-lifecycle task#1) and compare $.Id.
5. Cause 4: replay iot-managed-integrations-managed-thing-lifecycle step setup#1 and compare $.Id.
6. Cause 5: replay iot-managed-integrations-ota-task-configuration-lifecycle step setup#1 and compare $.TaskConfigurationId.
7. In each case the HTTP status matches and only the identifier member diverges; repeating the replay changes the served identifiers, because their leading 16 hex digits are the current wall clock.
