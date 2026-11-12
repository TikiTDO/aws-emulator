Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-control-policy-lifecycle
Issue-Step: task#5
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-control-policy-lifecycle
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|DeletePolicy|2xx->2xx|$.statusReasons[*]
Issue-Commit: a6732131b4577aead0e874dcd27f439fc83f19d6

# DeletePolicy returns an empty statusReasons list instead of the deletion reason

> _Note: this issue was identified against the code base at commit `a6732131b4577aead0e874dcd27f439fc83f19d6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeletePolicy

## Description
This bundle has 1 cause.

Cause 1 of 1 - DeletePolicy answers 202 with statusReasons serialized as an empty list, so the deletion reason AWS reports is absent (fixture bedrock-agentcore-control-policy-lifecycle, step task#5).

AWS: DeletePolicy is DELETE /policy-engines/{policyEngineId}/policies/{policyId} with @http code 202 (Smithy: DeletePolicy http {method: DELETE, uri: /policy-engines/{policyEngineId}/policies/{policyId}, code: 202}). Its output shape DeletePolicyResponse declares statusReasons (target PolicyStatusReasons, a list) as REQUIRED alongside policyId / name / policyEngineId / createdAt / updatedAt / policyArn / status / definition. Smithy documents DeletePolicyResponse$statusReasons as "Additional information about the deletion status. This provides details about the deletion process or any issues that may have occurred." The recorded response for a user-initiated delete of an existing policy carries exactly one reason entry whose text is the fixed string 'Deletion initiated by user' - it is constant text for this transition, with no request- or state-derived parts (recorded behavior for the reason text itself; the member and its requiredness come from the model).

Emulator: the status line and every other projected member match, but statusReasons comes back as an empty array, so there is no element 0 at all. Replay reports: "$.statusReasons[0]: missing (expected 'Deletion initiated by user', got None)" with expected_status 202 and actual_status 202.

Why: a client that surfaces or asserts on the deletion status reason - a console view, a cleanup script that logs why a policy went away, or a waiter that distinguishes a user-initiated delete from a service-side failure reason - reads an empty list and can report nothing. Because statusReasons is a required member, a caller is entitled to treat it as populated for the transition the response describes; serializing it empty makes every delete indistinguishable from one whose reason the service could not determine.

Fix: DeletePolicy's handler/state in the bedrock-agentcore-control crate - when the delete transition is recorded, push the user-initiated deletion reason onto the policy's status-reason collection so the DeletePolicy response projection emits it (the projection itself already carries the member, it is just never populated).

Traces: bedrock-agentcore-control-policy-lifecycle#task#5.

Related open issues that do NOT cover this: 20261108T204036Z-bedrock-agentcore-control-policy-policy-engine-and-api-key-provider-deletes.md covers the fact that a deleted policy / policy engine / API-key provider stays readable (the post-delete GET answering 200 instead of 404), and 20261108T204036Z-bedrock-agentcore-control-policy-policy-engine-and-gateway-rate-limit-timestamps.md covers epoch-number timestamps and rate-limit status settling. Neither names the DeletePolicy 202 response body's statusReasons member, and neither carries this step's replay key.

## Evidence
Replay of fixture bedrock-agentcore-control-policy-lifecycle, step task#5, operation DeletePolicy (divergence_key service=bedrock-agentcore-control|DeletePolicy|status=202->202|keys=$.statusReasons[0]):

Expected (recorded AWS response): HTTP 202, body member statusReasons = ['Deletion initiated by user'] (element [0] present, fixed text 'Deletion initiated by user').
Actual (emulator): HTTP 202, statusReasons = [] - replay detail: "$.statusReasons[0]: missing (expected 'Deletion initiated by user', got None)".

Status matches (202 -> 202); the only diverging key is $.statusReasons[0]. Model grounding: Smithy DeletePolicyResponse declares statusReasons (target PolicyStatusReasons, list) as required, documented as "Additional information about the deletion status. This provides details about the deletion process or any issues that may have occurred."

## Steps to Reproduce
1. Boot the emulator from this checkout and replay wire fixture bedrock-agentcore-control-policy-lifecycle.
2. The fixture creates a policy engine and a policy, then at step task#5 issues DELETE /policy-engines/{policyEngineId}/policies/{policyId} (DeletePolicy).
3. Inspect the 202 response body: statusReasons is []. AWS returns a one-element list whose entry is 'Deletion initiated by user'.
4. Re-replay bedrock-agentcore-control-policy-lifecycle#task#5 after the fix to confirm $.statusReasons[0] matches.
