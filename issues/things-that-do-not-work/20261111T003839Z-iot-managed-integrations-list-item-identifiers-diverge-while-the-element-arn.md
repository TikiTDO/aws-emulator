Issue-Source: trace-sweep-divergence
Issue-Trace: iot-managed-integrations-credential-locker-lifecycle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-managed-integrations-credential-locker-lifecycle iot-managed-integrations-managed-thing-lifecycle
Issue-Shape-Id: com.amazonaws.iotmanagedintegrations#IotManagedIntegrations
Issue-Key: iot-managed-integrations|ListCredentialLockers|2xx->2xx|$.Items[Arn="arn:aws:iotmanagedintegrations:eu-west-1:000000000000:credential-locker/18e6d6969efc80eb00000000"].Id
Issue-Key: iot-managed-integrations|ListManagedThings|2xx->2xx|$.Items[Arn="arn:aws:iotmanagedintegrations:eu-west-1:000000000000:managed-thing/18e6d694b2d0f9d900000000"].Id
Issue-Commit: b78c4779749c8e01cf02c8115fda52ca12207fbe

# List item identifiers diverge while the element ARN matches

> _Note: this issue was identified against the code base at commit `b78c4779749c8e01cf02c8115fda52ca12207fbe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListCredentialLockers

## Description
Two collection reads of iot-managed-integrations answer the modelled status 200 and the modelled body shape, yet the replay reports each list element diverging on exactly the member that carries the resource identifier (Items[].Id) while the SAME identifier embedded in that element's Arn is accepted (the Arn is what selected the element for comparison). Both values are produced by the same mint, `crate::state::opaque_id` in services/service-com-amazonaws-iotmanagedintegrations-iotmanagedintegrations/src/state.rs, which is 16 hex digits of SystemTime::now() nanoseconds plus an 8-hex-digit store counter, so the identifier changes on every replay. In both clusters the expected and the actual identifier RENDER identically in the replay detail, so the difference is not in the member's visible text but in the identifier's provenance/format. Hypothesis (neither the model nor the recording states this): the corpus pins Items[].Id to an identifier variable whose binding the emulator's clock+counter identifier does not satisfy, so the echo is reported again on each list step that serves it.

An open issue titled "Read and list responses diverge on the echoed resource identifier members" (20261110T214817Z-iot-managed-integrations-read-and-list-responses-diverge-on-the-echoed.md) describes this same family in prose, but the replay reports BOTH of these clusters as unowned: that issue's replay keys for the two list operations embed one run's ARN resource segment ("...credential-locker/18e6cd141522f84f00000000", "...managed-thing/18e6cd12208cbbd600000000"), and the mint produced different identifiers on this replay, so its keys no longer bind these steps. They are filed here so a fixer actually re-replays them.

This issue bundles TWO causes; both must be closed.

Cause 1 of 2: ListCredentialLockers (fixture iot-managed-integrations-credential-locker-lifecycle, step task#4).
AWS: Smithy: ListCredentialLockers -> GET /credential-lockers, success 200, paginated (inputToken/outputToken NextToken, items Items); output Items targets CredentialLockerListDefinition whose element CredentialLockerSummary carries Id (CredentialLockerId, @length 1..64, @pattern ^[a-zA-Z0-9]*$), Arn (CredentialLockerArn, @pattern ^arn:aws:iotmanagedintegrations:[0-9a-zA-Z-]+:[0-9]+:credential-locker/[0-9a-zA-Z]+$), Name and CreatedAt. AWS mints the locker identifier at CreateCredentialLocker and every later echo - the summary's Id and the resource segment of the summary's Arn - is that one identifier.
Emulator: 200 with the right element shape, but "$.Items[Arn=\"arn:aws:iotmanagedintegrations:<region>:<account>:credential-locker/<locker-id>\"].Id: value_mismatch" - inside ONE element the Arn is accepted while the Id member is not, even though both render the same 24-hex-character value.
Why: a client that enumerates lockers and keys its results by Items[].Id keys on a value the recorded API does not produce, while the identifier inside the same element's ARN is accepted - an intra-element inconsistency no AWS response has. Because the mint embeds the wall clock, the identifier also differs on every run, so nothing downstream can correlate a locker across replays.
Fix: the list projection for this operation (the Id member it copies out of the stored locker record) in the operation's handler/serializer, together with the identifier mint in src/state.rs `opaque_id`.
Traces: iot-managed-integrations-credential-locker-lifecycle#task#4.

Cause 2 of 2: ListManagedThings (fixture iot-managed-integrations-managed-thing-lifecycle, step task#3).
AWS: Smithy: ListManagedThings -> GET /managed-things, success 200, paginated, output Items targets ManagedThingListDefinition whose element ManagedThingSummary carries Id (ManagedThingId, @length 1..64, @pattern ^[a-zA-Z0-9:_-]*$) and Arn (ManagedThingArn, @pattern ^arn:aws:iotmanagedintegrations:[0-9a-zA-Z-]+:[0-9]+:managed-thing/([0-9a-zA-Z:_-])+$). The summary's Id is the handle later calls (GetManagedThing, UpdateManagedThing, DeleteManagedThing, device discovery, event-log configuration) address the thing by, and it is the identifier the Arn embeds.
Emulator: 200 with the right element shape, but "$.Items[Arn=\"arn:aws:iotmanagedintegrations:<region>:<account>:managed-thing/<managed-thing-id>\"].Id: value_mismatch" - the element matched on its Arn and diverged on its Id, the same intra-element inconsistency as Cause 1, with expected and actual rendering identically.
Why: clients enumerate managed things and then address them by Items[].Id; serving an identifier the recorded API rejects (and that changes every run) breaks list-then-act flows even though the ARN in the same element is accepted.
Fix: the list projection for this operation (the Id member copied out of the stored managed-thing record) in the operation's handler/serializer, together with the identifier mint in src/state.rs `opaque_id`.
Traces: iot-managed-integrations-managed-thing-lifecycle#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator booted from this checkout (emulator_up true, /__admin/health 200). Both clusters matched the expected status 200 and diverged only on the identifier member of a list element; neither carries a stub_path and the replay reports owned_by as EMPTY for both.

Cause 1 - ListCredentialLockers, fixture iot-managed-integrations-credential-locker-lifecycle, step task#4, expected status 200, actual status 200:
  divergence_key: service=iot-managed-integrations|ListCredentialLockers|status=200->200|keys=$.Items[Arn="arn:aws:iotmanagedintegrations:eu-west-1:000000000000:credential-locker/18e6d6969efc80eb00000000"].Id
  detail: $.Items[Arn="arn:aws:iotmanagedintegrations:eu-west-1:000000000000:credential-locker/18e6d6969efc80eb00000000"].Id: value_mismatch (expected '18e6d6969efc80eb00000000', got '18e6d6969efc80eb00000000')
  The element's Arn matched (it is the selector); only .Id is reported mismatched, and expected and actual render the same text.

Cause 2 - ListManagedThings, fixture iot-managed-integrations-managed-thing-lifecycle, step task#3, expected status 200, actual status 200:
  divergence_key: service=iot-managed-integrations|ListManagedThings|status=200->200|keys=$.Items[Arn="arn:aws:iotmanagedintegrations:eu-west-1:000000000000:managed-thing/18e6d694b2d0f9d900000000"].Id
  detail: $.Items[Arn="arn:aws:iotmanagedintegrations:eu-west-1:000000000000:managed-thing/18e6d694b2d0f9d900000000"].Id: value_mismatch (expected '18e6d694b2d0f9d900000000', got '18e6d694b2d0f9d900000000')

The region (eu-west-1), the account (000000000000) and the 24-hex identifiers above are this run's values, not requirements: the requirement is that the element's Id is the identifier the create minted and the one the element's own ARN embeds, matching CredentialLockerId / ManagedThingId. Every identifier above has the structure src/state.rs `opaque_id` emits: format!("{:016x}{counter:08x}", SystemTime::now() nanos as u64), so it is clock-derived and differs on every replay.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture iot-managed-integrations-credential-locker-lifecycle through step task#4 (CreateCredentialLocker, then GET /credential-lockers). Select the element by its Arn and compare its .Id to the locker identifier the fixture pinned at create time and to the resource segment of the element's own Arn.
3. Cause 2: replay fixture iot-managed-integrations-managed-thing-lifecycle through step task#3 (CreateManagedThing, then GET /managed-things). Select the element by its Arn and compare its .Id the same way.
4. Both steps return status 200 and the modelled element shape; only Items[].Id is reported diverging, while the identifier inside the same element's Arn is accepted. Re-run the replay: the identifiers change, because the mint embeds the wall clock.
