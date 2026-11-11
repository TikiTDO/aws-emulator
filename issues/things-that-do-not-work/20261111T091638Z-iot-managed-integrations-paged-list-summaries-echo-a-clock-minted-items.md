Issue-Source: trace-sweep-divergence
Issue-Trace: iot-managed-integrations-credential-locker-lifecycle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-managed-integrations-credential-locker-lifecycle iot-managed-integrations-managed-thing-lifecycle
Issue-Shape-Id: com.amazonaws.iotmanagedintegrations#IotManagedIntegrations
Issue-Key: iot-managed-integrations|ListCredentialLockers|2xx->2xx|$.Items[Arn="arn:aws:iotmanagedintegrations:eu-west-1:000000000000:credential-locker/18e6f2d023a5b50700000000"].Id
Issue-Key: iot-managed-integrations|ListManagedThings|2xx->2xx|$.Items[Arn="arn:aws:iotmanagedintegrations:eu-west-1:000000000000:managed-thing/18e6f2ce357f12ba00000000"].Id
Issue-Commit: fb69d3be166efc81cd2d34b9ca1530fe0e609c7b

# Paged list summaries echo a clock-minted Items[].Id that no replay can rebind

> _Note: this issue was identified against the code base at commit `fb69d3be166efc81cd2d34b9ca1530fe0e609c7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListCredentialLockers

## Description
Two collection reads of iot-managed-integrations answer the modelled status 200 and the modelled element shape, yet in each served list element exactly ONE member is reported diverging: Items[].Id, the member that carries the resource identifier. The identifier embedded in the SAME element's Arn is accepted - the Arn is the selector the replay used to pick the element - so the divergence is intra-element: within one summary structure the ARN form of the value passes and the Id member does not, even though the two render identical text.

Root cause: both the stored id and the stored ARN come from one mint, `crate::state::opaque_id` (services/service-com-amazonaws-iotmanagedintegrations-iotmanagedintegrations/src/state.rs lines 1997-2006), which is `format!("{:016x}{counter:08x}", SystemTime::now() nanos as u64)` - 16 hex digits of the wall clock plus an 8-hex-digit per-store counter. Neither list handler transforms it: list_credential_lockers.rs copies `locker.id` and `locker.arn` straight out of the record, list_managed_things.rs copies `thing.id` and `thing.arn` the same way. The identifier is therefore different on every run, which is also why the two prior issues filed for exactly these steps no longer bind them (see below).

Hypothesis (neither the Smithy model nor the recording states this): the corpus pins Items[].Id to an identifier variable whose binding a clock+counter identifier does not satisfy, and only that member is pinned that way, which is why the same value inside the element's ARN is accepted.

Already-filed issues that describe this family but do NOT own these clusters (the replay reports owned_by EMPTY for both): "Read and list responses diverge on the echoed resource identifier members" (20261110T214817Z-iot-managed-integrations-read-and-list-responses-diverge-on-the-echoed.md), "List item identifiers diverge while the element ARN matches" (20261111T003839Z-iot-managed-integrations-list-item-identifiers-diverge-while-the-element-arn.md) and "Collection reads serve an Items[].Id the recorded API rejects" (20261111T034608Z-iot-managed-integrations-collection-reads-serve-an-items-id-the-recorded.md). Every one of those issues' replay keys for the two list operations embeds ONE run's ARN resource segment, and because the mint embeds the wall clock this replay produced different identifiers, so none of those keys matches these steps any more. That key instability is itself part of the defect: while the identifier is clock-derived, no ARN-bearing replay key recorded for these steps survives a re-run, so a fixer must confirm by re-replaying both fixtures end to end rather than by key equality. The service's other filed divergences (DeleteCloudConnector 404, DeleteOtaTaskConfiguration wording, UpdateManagedThing Content-Length, the custom-endpoint address, the create-side identifier mint) are different operations or different members and do not carry these replay keys.

This issue bundles TWO causes; both must be closed.

Cause 1 of 2: ListCredentialLockers (fixture iot-managed-integrations-credential-locker-lifecycle, step task#4).
AWS: Smithy: ListCredentialLockers -> GET /credential-lockers, success 200, readonly, paginated (inputToken/outputToken NextToken, pageSize MaxResults, items Items); output Items targets CredentialLockerListDefinition whose element CredentialLockerSummary carries Id (CredentialLockerId, string, @length 1..64, @pattern ^[a-zA-Z0-9]*$, doc "The id of the credential locker"), Arn (CredentialLockerArn, @length 32..1011, @pattern ^arn:aws:iotmanagedintegrations:[0-9a-zA-Z-]+:[0-9]+:credential-locker/[0-9a-zA-Z]+$), Name and CreatedAt. AWS mints the locker identifier once, at CreateCredentialLocker, and every later echo - the summary's Id and the resource segment of the summary's own Arn - is that one identifier.
Emulator: 200 with the right element shape, but "$.Items[Arn=\"arn:aws:iotmanagedintegrations:<region>:<account>:credential-locker/<locker-id>\"].Id: value_mismatch" - inside ONE element the Arn is accepted while the Id member is not; expected and actual render the same 24-hex-character value, which has exactly the shape src/state.rs opaque_id emits.
Why: a client that enumerates lockers and keys its results by Items[].Id keys on a value the recorded API does not produce, while the identifier inside the same element's ARN is accepted - an intra-element inconsistency no AWS response has. Because the mint embeds the wall clock the identifier also changes every run, so nothing downstream can correlate a locker across replays.
Fix: the list projection for this operation - the Id member it copies out of the stored locker record in the operation's handler - together with the identifier mint in src/state.rs `opaque_id`.
Traces: iot-managed-integrations-credential-locker-lifecycle#task#4.

Cause 2 of 2: ListManagedThings (fixture iot-managed-integrations-managed-thing-lifecycle, step task#3).
AWS: Smithy: ListManagedThings -> GET /managed-things, success 200, readonly, paginated; output Items targets ManagedThingListDefinition whose element ManagedThingSummary carries Id (ManagedThingId, string, @length 1..64, @pattern ^[a-zA-Z0-9:_-]*$, doc "The id of the device") and Arn (ManagedThingArn, @length 32..1011, @pattern ^arn:aws:iotmanagedintegrations:[0-9a-zA-Z-]+:[0-9]+:managed-thing/([0-9a-zA-Z:_-])+$). The summary's Id is the handle later calls (GetManagedThing, UpdateManagedThing, DeleteManagedThing, device discovery, event-log configuration) address the thing by, and it is the identifier its own ARN embeds.
Emulator: 200 with the right element shape, but "$.Items[Arn=\"arn:aws:iotmanagedintegrations:<region>:<account>:managed-thing/<managed-thing-id>\"].Id: value_mismatch" - the element matched on its Arn and diverged on its Id, the same intra-element inconsistency as Cause 1, with expected and actual rendering identically.
Why: clients enumerate managed things and then address them by Items[].Id; serving an identifier the recorded API rejects, and that changes on every run, breaks list-then-act flows even though the ARN in the same element is accepted.
Fix: the list projection for this operation - the Id member copied out of the stored managed-thing record in the operation's handler - together with the identifier mint in src/state.rs `opaque_id`.
Traces: iot-managed-integrations-managed-thing-lifecycle#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator booted from this checkout: emulator_up true, "/__admin/health answered 200 and the :40893 data listener accepts", 89 steps total, 48 passed, score 53.9%, 17 divergence clusters. Both clusters below matched the expected status 200, carry NO stub_path, and the replay reports owned_by as EMPTY for both; every other iot-managed-integrations divergence in this run is owned or stub-blocked.

Cause 1 - ListCredentialLockers, fixture iot-managed-integrations-credential-locker-lifecycle, step task#4, expected status 200, actual status 200:
  divergence_key: service=iot-managed-integrations|ListCredentialLockers|status=200->200|keys=$.Items[Arn="arn:aws:iotmanagedintegrations:eu-west-1:000000000000:credential-locker/18e6f2d023a5b50700000000"].Id
  detail: $.Items[Arn="arn:aws:iotmanagedintegrations:eu-west-1:000000000000:credential-locker/18e6f2d023a5b50700000000"].Id: value_mismatch (expected '18e6f2d023a5b50700000000', got '18e6f2d023a5b50700000000')
  The element's Arn matched (it is the selector); only .Id is reported mismatched, and expected and actual render the same text. The same fixture's CreateCredentialLocker step (task#1) is reported diverging on $.Id and $.Arn against the corpus's {{locker_id}} variable, so the identifier this list echoes is the one the create already minted wrongly.

Cause 2 - ListManagedThings, fixture iot-managed-integrations-managed-thing-lifecycle, step task#3, expected status 200, actual status 200:
  divergence_key: service=iot-managed-integrations|ListManagedThings|status=200->200|keys=$.Items[Arn="arn:aws:iotmanagedintegrations:eu-west-1:000000000000:managed-thing/18e6f2ce357f12ba00000000"].Id
  detail: $.Items[Arn="arn:aws:iotmanagedintegrations:eu-west-1:000000000000:managed-thing/18e6f2ce357f12ba00000000"].Id: value_mismatch (expected '18e6f2ce357f12ba00000000', got '18e6f2ce357f12ba00000000')
  The same fixture's CreateManagedThing step (setup#1) is reported diverging on $.Id against {{managed_thing_id}}.

Code: services/service-com-amazonaws-iotmanagedintegrations-iotmanagedintegrations/src/state.rs lines 1997-2006 define `opaque_id(counter)` as format!("{:016x}{counter:08x}", SystemTime::now() nanos as u64). src/ops/list_credential_lockers.rs lines 188-197 build each CredentialLockerSummary with `id: Some(locker.id.clone())` and `arn: Some(locker.arn.clone())`; src/ops/list_managed_things.rs lines 257-285 build each ManagedThingSummary with `id: Some(thing.id.clone())` and `arn: Some(thing.arn.clone())` - so the Id a list serves is exactly the stored mint value, and the ARN embeds the same value.

The region (eu-west-1), the account (000000000000) and the 24-hex identifiers quoted above are THIS run's values, not requirements. The requirement is that each element's Id is the identifier the create minted and the one the element's own ARN embeds, satisfying CredentialLockerId (@length 1..64, @pattern ^[a-zA-Z0-9]*$) / ManagedThingId (@length 1..64, @pattern ^[a-zA-Z0-9:_-]*$).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
2. Cause 1: replay fixture iot-managed-integrations-credential-locker-lifecycle through step task#4 - CreateCredentialLocker (task#1), then GET /credential-lockers. Select the served element by its Arn and compare its .Id both to the locker identifier the fixture pinned at create time and to the resource segment of the element's own Arn.
3. Cause 2: replay fixture iot-managed-integrations-managed-thing-lifecycle through step task#3 - CreateManagedThing (setup#1), then GET /managed-things. Select the element by its Arn and compare its .Id the same way.
4. Both steps return status 200 and the modelled element shape; only Items[].Id is reported diverging, while the identifier inside the same element's Arn is accepted.
5. Re-run the whole replay: the identifiers change between runs because src/state.rs `opaque_id` embeds the wall clock, so any ARN-bearing replay key recorded for these steps stops matching. Confirm a fix by re-replaying BOTH fixtures end to end, not by key equality.
