Issue-Source: trace-sweep-divergence
Issue-Trace: securityagent-agent-space-errors
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: securityagent-agent-space-errors securityagent-agent-space-lifecycle securityagent-agent-space-target-domain-association
Issue-Shape-Id: com.amazonaws.securityagent#SecurityAgent
Issue-Key: securityagent|UpdateAgentSpace|4xx->4xx|$.message
Issue-Key: securityagent|UpdateAgentSpace|2xx->2xx|$.arn
Issue-Key: securityagent|UpdateAgentSpace|2xx->2xx|$.id
Issue-Key: securityagent|UpdateAgentSpace|2xx->2xx|$.ownerId
Issue-Key: securityagent|UpdateAgentSpace|2xx->2xx|$.targetDomainIds
Issue-Commit: cbe72886ae20fcfe92527c91c9a68d30188059bc

# UpdateAgentSpace responses omit agent-instance identity fields and its not-found message diverges

> _Note: this issue was identified against the code base at commit `cbe72886ae20fcfe92527c91c9a68d30188059bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAgentSpace

## Description
Three net-new causes, all on securityagent UpdateAgentSpace: the emulator reaches the right HTTP status in every case but its 404 message text is its own wording, and its 200 response body drops the agent-instance identity fields AWS returns (and emits an empty list where AWS omits the member). Each cause must be closed separately; they are listed below with their own fixture and step.

Note on related open issues: 20261029T204635Z-securityagent-error-message-text-diverges-from-aws-for.md describes the same message-text defect CLASS for AddArtifact and CreatePrivateConnection, and 20261029T203228Z-securityagent-batchgetagentspaces-returns-stale-incomplete-agent-space-state.md covers response-shape gaps on BatchGetAgentSpaces. Neither covers UpdateAgentSpace, and the replay suite did not mark any of these three clusters as owned by them.

Cause 1 of 3: UpdateAgentSpace not-found rejection emits emulator wording that echoes the requested id (fixture securityagent-agent-space-errors, step task#2).
AWS: Smithy: UpdateAgentSpace (POST /UpdateAgentSpace, restJson1, success code 200 defaulted) declares input member agentSpaceId as required and binds no operation-specific errors; the service's ResourceNotFoundException is a client error with httpError 404 and a required `message`. For an agentSpaceId that names no agent space, the recorded response is HTTP 404 with message "The specified agent instance does not exist." — that whole sentence is FIXED text; AWS does NOT interpolate the requested identifier, so this message has no variable parts.
Emulator: HTTP 404 (correct) but message "No agent space found with id as-doesnotexist0000" — different fixed text, and it appends the agentSpaceId taken from the request ('as-doesnotexist0000' is just that run's request value, not part of any requirement).
Why: clients, SDK error handlers and assertions that match on the documented not-found text for this operation do not match, and the emulator leaks a request value into a message AWS keeps free of it, so the emulated 404 is trivially distinguishable from the real one.
Fix: the UpdateAgentSpace handler's not-found error construction — build the ResourceNotFoundException message from this operation's fixed sentence instead of formatting the looked-up id into a local string.
Traces: securityagent-agent-space-errors#task#2.

Cause 2 of 3: successful UpdateAgentSpace omits the agent-instance identity members id, ownerId and arn (fixture securityagent-agent-space-lifecycle, step task#4).
AWS: recorded behavior only — the modeled UpdateAgentSpaceOutput declares agentSpaceId (required), name (required), description, awsResources, targetDomainIds, codeReviewSettings, createdAt and updatedAt, and says nothing about identity fields; the recording shows AWS's 200 body ALSO carrying `id` (the agent-instance identifier, equal to the agent space id), `ownerId` (the 12-digit owning account id) and `arn` (arn:aws:securityagent:<region>:<account>:agent-instance/<id>). The recorded values of those fields are environment-specific; the requirement is that the three members are present and self-consistent with the updated resource.
Emulator: HTTP 200 with the rest of the body, but $.id, $.ownerId and $.arn are absent (replay reports them as missing).
Why: a client that reads the ARN or owner account from the update response — to tag, to build a follow-up ARN-keyed call, or to confirm which instance it just mutated — gets null and fails or silently mis-targets, even though the mutation itself succeeded.
Fix: the UpdateAgentSpace output serializer (the operation's response-building code, which must project the stored agent space's instance id, owner account and ARN alongside the modeled members).
Traces: securityagent-agent-space-lifecycle#task#4.

Cause 3 of 3: same identity omission after a target-domain association, plus targetDomainIds serialized as an empty array where AWS omits the member (fixture securityagent-agent-space-target-domain-association, step task#4).
AWS: recorded behavior only — as in cause 2 the 200 body carries id, ownerId and arn. For targetDomainIds (optional TargetDomainIdList) the recording shows the member ABSENT from the response when the update does not carry target domain ids; AWS does not substitute an empty array.
Emulator: HTTP 200 with $.id, $.ownerId and $.arn missing, and $.targetDomainIds present as [] where the recorded response has no such member at all.
Why: beyond the identity gap of cause 2, a client distinguishing "not returned" from "returned empty" reads the emulator's [] as an authoritative statement that the agent space has no target domains, which is a different fact from the field being unset — association state inferred from the update response is therefore wrong.
Fix: the UpdateAgentSpace output serializer / its state projection — populate the identity members, and suppress optional list members that have no value rather than emitting an empty array.
Traces: securityagent-agent-space-target-domain-association#task#4.

## Evidence
Cause 1 (fixture securityagent-agent-space-errors, step task#2, UpdateAgentSpace, expected 404 / actual 404): $.message value_mismatch.
  expected: "The specified agent instance does not exist."  (entirely fixed text)
  actual:   "No agent space found with id as-doesnotexist0000"  (emulator wording + the agentSpaceId from the request; the id shown is that run's value and not part of the requirement)
Status and error shape (ResourceNotFoundException, client, httpError 404 per the Smithy model) match; only the message text diverges.

Cause 2 (fixture securityagent-agent-space-lifecycle, step task#4, UpdateAgentSpace, expected 200 / actual 200):
  $.id: missing (expected the agent-instance identifier, '00000000000000000000000000000001' in this run, got None)
  $.ownerId: missing (expected the owning 12-digit account id, '000000000000' in this run, got None)
  $.arn: missing (expected 'arn:aws:securityagent:us-west-2:000000000000:agent-instance/<id>', got None)
The per-run id, account and region values above only fill the template; the requirement is that the three members are emitted and consistent with the updated agent space.

Cause 3 (fixture securityagent-agent-space-target-domain-association, step task#4, UpdateAgentSpace, expected 200 / actual 200):
  $.id / $.ownerId / $.arn: missing, exactly as in cause 2
  $.targetDomainIds: type_mismatch (expected None — the member is absent from the recorded AWS response — got [])

Model references used: smithy_query query=operation operation=UpdateAgentSpace on services/service-com-amazonaws-securityagent-securityagent/smithy-model/com-amazonaws-securityagent-securityagent.json (input agentSpaceId required; output members agentSpaceId/name required, targetDomainIds optional TargetDomainIdList; http POST /UpdateAgentSpace code 200 defaulted; no operation-bound errors) and query=shape shape=ResourceNotFoundException (client, httpError 404, required message).

## Steps to Reproduce
1. git_clone() the repo and boot the emulator built from that checkout, then replay_wire_corpus().
2. Cause 1: replay fixture securityagent-agent-space-errors step task#2 — POST /UpdateAgentSpace with agentSpaceId set to an id that does not exist. The response is 404 (correct) but Error.Message is "No agent space found with id <requested id>" instead of AWS's fixed "The specified agent instance does not exist.".
3. Cause 2: replay fixture securityagent-agent-space-lifecycle step task#4 — CreateAgentSpace, then UpdateAgentSpace on that space (description / codeReviewSettings change). The 200 body lacks id, ownerId and arn.
4. Cause 3: replay fixture securityagent-agent-space-target-domain-association step task#4 — create an agent space, create and associate a target domain, then UpdateAgentSpace on that space. The 200 body again lacks id, ownerId and arn, and additionally contains "targetDomainIds": [] where the recorded AWS response omits the member.
