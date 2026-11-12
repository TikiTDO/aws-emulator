Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.capability-invalid-name-rejected
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.capability-invalid-name-rejected|_client_error#4|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: 09fb1164602e6ebdd2bd6e78b61b67ab44fe1e77

# opensearch RegisterCapability accepts any well-formed capabilityName instead of refusing everything but ai-capability

> _Note: this issue was identified against the code base at commit `09fb1164602e6ebdd2bd6e78b61b67ab44fe1e77`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterCapability

## Description
Cause 1 of 1: RegisterCapability performs no value validation on `capabilityName`, so a well-formed but unsupported name is registered as a real capability instead of being refused.

AWS: RegisterCapability (POST /2021-01-01/opensearch/application/{applicationId}/capability/register) models `capabilityName` as CapabilityName with @length min 3 max 30 and @pattern ^[a-zA-Z0-9-]+$, and the member's own documentation fixes the set of accepted values: "This identifies the type of capability being enabled for the application. For registering AI Assistant capability, use ai-capability" (Smithy: RegisterCapabilityRequest.capabilityName; modeled errors include ValidationException, HTTP 400). Recorded behavior: a name that satisfies the length and pattern constraints but is not `ai-capability` is refused with HTTP 400, error code `ValidationException`, and the message `Invalid capability name for AI Capability: <capabilityName>. Only ai-capability is allowed` — the fixed text is `Invalid capability name for AI Capability: ` and `. Only ai-capability is allowed`, and the only variable part is the `capabilityName` value copied from the request (in the recording, `not-ai-capability`). Nothing is created: a subsequent GetCapability for that name is refused with `ResourceNotFoundException` and the message `Capability <capabilityName> not found for application <applicationId>` (fixed text `Capability ` / ` not found for application `, with the capability name and the application id taken from the request). The same application accepts `ai-capability`, which is minted in `creating` and reaches `active`.

Emulator: the call SUCCEEDS. It answers HTTP 200 with a registered record under the unsupported name — the replay shows `{'capabilityName': 'not-ai-capability', 'applicationId': 'sfi9sv1baaaa', 'status': 'creating', 'capabilityConfig': {'aiConfig': {}}}` — and stores that capability on the application, so GetCapability/ListCapabilities then report a capability AWS never created. No ValidationException is produced for any value of `capabilityName` that merely matches the modeled length/pattern.

Why: a client relying on AWS's behavior treats a successful RegisterCapability as proof that the named capability type exists and is being enabled. Against the emulator a typo'd or unsupported capability name is silently accepted and reaches `creating`/`active`, so code that probes for supported capability types, or that validates user-supplied capability names by calling the API, concludes the wrong thing and never surfaces the 400 it must handle. Negative-path tests that assert the refusal and the absence of the record cannot pass, and the application's capability set is polluted with records that have no counterpart in real AWS.

Fix: the RegisterCapability handler's input validation — the value check belongs ahead of the application lookup/quota logic in the operation's handle body, refusing a `capabilityName` other than `ai-capability` with the modeled ValidationException (HTTP 400) carrying the message above, and leaving the application's capability map untouched so the effect-absent read still answers ResourceNotFoundException.

Traces: opensearch.capability-invalid-name-rejected. Handler: services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/register_capability.rs (handle, lines 187-253: the only refusals raised are application-not-found and the capability quota; `input.capability_name` is used as a map key at lines 221-251 without any value check).

## Evidence
Check opensearch.capability-invalid-name-rejected (mismatch).

Expected (AWS, per the recording and the Smithy member doc):
  RegisterCapability(applicationId=<app>, capabilityName="not-ai-capability", capabilityConfig={"aiConfig": {}})
  -> HTTP 400, Error.Code = "ValidationException",
     Error.Message = "Invalid capability name for AI Capability: not-ai-capability. Only ai-capability is allowed"
  then GetCapability(applicationId=<app>, capabilityName="not-ai-capability")
  -> HTTP 409/ResourceNotFoundException, Message = "Capability not-ai-capability not found for application <app>"

Actual (emulator):
  RegisterCapability returned HTTP 200:
    {'ResponseMetadata': {'HTTPStatusCode': 200, ...},
     'capabilityName': 'not-ai-capability', 'applicationId': 'sfi9sv1baaaa',
     'status': 'creating', 'capabilityConfig': {'aiConfig': {}}}
  Harness assertion:
    AssertionError: expected a rejection, but the call SUCCEEDED and returned {...'capabilityName': 'not-ai-capability'...}
    (check_opensearch_capability_invalid_name_rejected.py line 23 -> _client_error, line 71)

Smithy evidence: RegisterCapabilityRequest.capabilityName -> target CapabilityName, required, length 3..30, pattern ^[a-zA-Z0-9-]+$, doc "...For registering AI Assistant capability, use ai-capability"; RegisterCapability errors include ValidationException (httpError 400) and ResourceNotFoundException.

## Steps to Reproduce
1. CreateApplication(name=<unique lowercase name>) against the emulator's opensearch endpoint and poll GetApplication until status == ACTIVE; keep the returned id as <app>.
2. Call RegisterCapability(applicationId=<app>, capabilityName="not-ai-capability", capabilityConfig={"aiConfig": {}}).
   Expected: HTTP 400 ValidationException with message "Invalid capability name for AI Capability: not-ai-capability. Only ai-capability is allowed".
   Observed: HTTP 200 with a registered capability record (status 'creating') under that name.
3. Call GetCapability(applicationId=<app>, capabilityName="not-ai-capability").
   Expected: ResourceNotFoundException "Capability not-ai-capability not found for application <app>" (the refused register created nothing).
   Observed: the capability is returned, proving the record was stored.
4. Positive control: RegisterCapability(applicationId=<app>, capabilityName="ai-capability", capabilityConfig={"aiConfig": {}}) must still be accepted and reach 'active'.
Replay: run the behavioral suite check opensearch.capability-invalid-name-rejected.
