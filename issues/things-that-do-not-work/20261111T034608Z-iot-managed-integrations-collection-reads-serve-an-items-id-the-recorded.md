Issue-Source: trace-sweep-divergence
Issue-Trace: iot-managed-integrations-credential-locker-lifecycle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-managed-integrations-credential-locker-lifecycle iot-managed-integrations-managed-thing-lifecycle
Issue-Shape-Id: com.amazonaws.iotmanagedintegrations#IotManagedIntegrations
Issue-Key: iot-managed-integrations|ListCredentialLockers|2xx->2xx|$.Items[Arn="arn:aws:iotmanagedintegrations:eu-west-1:000000000000:credential-locker/18e6e0cf873c2a7300000000"].Id
Issue-Key: iot-managed-integrations|ListManagedThings|2xx->2xx|$.Items[Arn="arn:aws:iotmanagedintegrations:eu-west-1:000000000000:managed-thing/18e6e0cd96fa70f600000000"].Id
Issue-Commit: 6b0a119ee0c782fe5cb0865ca1b5cb8bebcdd5f6

# Collection reads serve an Items[].Id the recorded API rejects

> _Note: this issue was identified against the code base at commit `6b0a119ee0c782fe5cb0865ca1b5cb8bebcdd5f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListCredentialLockers

## Description
Two collection reads of iot-managed-integrations return the modelled status 200 and the modelled element shape, yet in each served list element exactly one member diverges: Items[].Id, the member carrying the resource identifier. The SAME identifier embedded in that element's Arn is accepted (the Arn is the selector the replay used to pick the element), so the divergence is intra-element: within one summary structure the ARN is accepted and the Id is not, even though the two render the same text. Both values come from one mint, `crate::state::opaque_id` in services/service-com-amazonaws-iotmanagedintegrations-iotmanagedintegrations/src/state.rs, which is 16 hex digits of SystemTime::now() nanoseconds plus an 8-hex-digit store counter, so the identifier is clock-derived and differs on every replay. Hypothesis (neither the Smithy model nor the recording states this): the corpus pins Items[].Id to an identifier variable whose binding a clock+counter identifier does not satisfy, so the echo is reported again on each list step that serves it; only the Id member is pinned that way, which is why the ARN form of the same value passes.

Two open issues describe this family in prose but the replay reports BOTH of these clusters with owned_by EMPTY, i.e. neither issue's replay keys bind these steps any more: "Read and list responses diverge on the echoed resource identifier members" (20261110T214817Z-iot-managed-integrations-read-and-list-responses-diverge-on-the-echoed.md) and "List item identifiers diverge while the element ARN matches" (20261111T003839Z-iot-managed-integrations-list-item-identifiers-diverge-while-the-element-arn.md). Both of those issues' keys for the two list operations embed ONE run's ARN resource segment ("...credential-locker/18e6cd141522f84f00000000" / "...credential-locker/18e6d6969efc80eb00000000", "...managed-thing/18e6cd12208cbbd600000000" / "...managed-thing/18e6d694b2d0f9d900000000"); because the mint embeds the wall clock, this replay produced different identifiers and those keys no longer match. That key instability is itself part of the defect: as long as the identifier is clock-derived, no filed replay key for these steps survives a re-run. A fix must make the list element's Id the create-minted, corpus-accepted identifier (the same value the element's own ARN embeds) rather than a per-run clock value.

This issue bundles TWO causes; both must be closed.

Cause 1 of 2: ListCredentialLockers (fixture iot-managed-integrations-credential-locker-lifecycle, step task#4).
AWS: Smithy: ListCredentialLockers -> GET /credential-lockers, success 200, paginated (inputToken/outputToken NextToken, pageSize MaxResults, items Items), readonly; output Items targets CredentialLockerListDefinition whose element CredentialLockerSummary carries Id (CredentialLockerId, string, @length 1..64, @pattern ^[a-zA-Z0-9]*$, doc "The id of the credential locker"), Arn (CredentialLockerArn, @length 32..1011, @pattern ^arn:aws:iotmanagedintegrations:[0-9a-zA-Z-]+:[0-9]+:credential-locker/[0-9a-zA-Z]+$), Name and CreatedAt. AWS mints the locker identifier once, at CreateCredentialLocker, and every later echo - the summary's Id and the resource segment of the summary's own Arn - is that one identifier.
Emulator: 200 with the right element shape, but "$.Items[Arn=\"arn:aws:iotmanagedintegrations:<region>:<account>:credential-locker/<locker-id>\"].Id: value_mismatch" - inside ONE element the Arn is accepted while the Id member is not; expected and actual render the same 24-hex-character value, which has exactly the shape src/state.rs opaque_id emits.
Why: a client that enumerates lockers and keys its results by Items[].Id keys on a value the recorded API does not produce, while the identifier inside the same element's ARN is accepted - an intra-element inconsistency no AWS response has. Because the mint embeds the wall clock, the identifier also changes on every run, so nothing downstream (and no filed replay key) can correlate a locker across replays.
Fix: the list projection for this operation - the Id member it copies out of the stored locker record in the operation's handler/serializer - together with the identifier mint in src/state.rs `opaque_id`.
Traces: iot-managed-integrations-credential-locker-lifecycle#task#4.

Cause 2 of 2: ListManagedThings (fixture iot-managed-integrations-managed-thing-lifecycle, step task#3).
AWS: Smithy: ListManagedThings -> GET /managed-things, success 200, paginated, readonly; output Items targets ManagedThingListDefinition whose element ManagedThingSummary carries Id (ManagedThingId, string, @length 1..64, @pattern ^[a-zA-Z0-9:_-]*$, doc "The id of the device") and Arn (ManagedThingArn, @length 32..1011, @pattern ^arn:aws:iotmanagedintegrations:[0-9a-zA-Z-]+:[0-9]+:managed-thing/([0-9a-zA-Z:_-])+$). The summary's Id is the handle later calls (GetManagedThing, UpdateManagedThing, DeleteManagedThing, device discovery, event-log configuration) address the thing by, and it is the identifier its ARN embeds.
Emulator: 200 with the right element shape, but "$.Items[Arn=\"arn:aws:iotmanagedintegrations:<region>:<account>:managed-thing/<managed-thing-id>\"].Id: value_mismatch" - the element matched on its Arn and diverged on its Id, the same intra-element inconsistency as Cause 1, with expected and actual rendering identically.
Why: clients enumerate managed things and then address them by Items[].Id; serving an identifier the recorded API rejects (and that changes every run) breaks list-then-act flows even though the ARN in the same element is accepted.
Fix: the list projection for this operation - the Id member copied out of the stored managed-thing record in the operation's handler/serializer - together with the identifier mint in src/state.rs `opaque_id`.
Traces: iot-managed-integrations-managed-thing-lifecycle#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator booted from this checkout: emulator_up true, /__admin/health answered 200, 89 steps replayed, score 53.9%. Both clusters below matched the expected status 200 and diverged only on the identifier member of a list element; neither carries a stub_path, and the replay reports owned_by as EMPTY for both (every other iot-managed-integrations divergence in this run is owned or stub-blocked).

Cause 1 - ListCredentialLockers, fixture iot-managed-integrations-credential-locker-lifecycle, step task#4, expected status 200, actual status 200:
  divergence_key: service=iot-managed-integrations|ListCredentialLockers|status=200->200|keys=$.Items[Arn="arn:aws:iotmanagedintegrations:eu-west-1:000000000000:credential-locker/18e6e0cf873c2a7300000000"].Id
  detail: $.Items[Arn="arn:aws:iotmanagedintegrations:eu-west-1:000000000000:credential-locker/18e6e0cf873c2a7300000000"].Id: value_mismatch (expected '18e6e0cf873c2a7300000000', got '18e6e0cf873c2a7300000000')
  The element's Arn matched (it is the selector); only .Id is reported mismatched, and expected and actual render the same text. The same fixture's CreateCredentialLocker step (task#1) is reported diverging on $.Id and $.Arn against the corpus's {{locker_id}} variable, so the identifier this list echoes is the one the create already minted wrongly.

Cause 2 - ListManagedThings, fixture iot-managed-integrations-managed-thing-lifecycle, step task#3, expected status 200, actual status 200:
  divergence_key: service=iot-managed-integrations|ListManagedThings|status=200->200|keys=$.Items[Arn="arn:aws:iotmanagedintegrations:eu-west-1:000000000000:managed-thing/18e6e0cd96fa70f600000000"].Id
  detail: $.Items[Arn="arn:aws:iotmanagedintegrations:eu-west-1:000000000000:managed-thing/18e6e0cd96fa70f600000000"].Id: value_mismatch (expected '18e6e0cd96fa70f600000000', got '18e6e0cd96fa70f600000000')
  The same fixture's CreateManagedThing step (setup#1) is reported diverging on $.Id against {{managed_thing_id}}.

The region (eu-west-1), the account (000000000000) and the 24-hex identifiers quoted above are THIS run's values, not requirements. The requirement is that the element's Id is the identifier the create minted and the one the element's own ARN embeds, satisfying CredentialLockerId (@pattern ^[a-zA-Z0-9]*$, 1..64) / ManagedThingId (@pattern ^[a-zA-Z0-9:_-]*$, 1..64). Every identifier above has the structure src/state.rs `opaque_id` emits - format!("{:016x}{counter:08x}", SystemTime::now() nanos as u64) - so it is clock-derived and differs on every replay, which is also why the replay keys of the two prior issues for these same steps no longer bind them.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
2. Cause 1: replay fixture iot-managed-integrations-credential-locker-lifecycle through step task#4 - CreateCredentialLocker (task#1), then GET /credential-lockers. Select the served element by its Arn and compare its .Id both to the locker identifier the fixture pinned at create time and to the resource segment of the element's own Arn.
3. Cause 2: replay fixture iot-managed-integrations-managed-thing-lifecycle through step task#3 - CreateManagedThing (setup#1), then GET /managed-things. Select the element by its Arn and compare its .Id the same way.
4. Both steps return status 200 and the modelled element shape; only Items[].Id is reported diverging, while the identifier inside the same element's Arn is accepted.
5. Re-run the whole replay: the identifiers change between runs because the mint embeds the wall clock, so any ARN-bearing replay key recorded for these steps stops matching - confirm a fix by re-replaying both fixtures end to end rather than by key equality.
