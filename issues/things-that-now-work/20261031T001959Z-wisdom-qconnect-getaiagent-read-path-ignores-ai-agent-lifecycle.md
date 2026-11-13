Issue-Source: trace-sweep-divergence
Issue-Trace: qconnect-ai-agent-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: qconnect-ai-agent-lifecycle
Issue-Shape-Id: com.amazonaws.qconnect#WisdomService
Issue-Key: wisdom|GetAIAgent|2xx->2xx|$.aiAgent.aiAgentArn
Issue-Key: wisdom|GetAIAgent|2xx->2xx|$.aiAgent.description
Issue-Key: wisdom|GetAIAgent|2xx->2xx|$.aiAgent.visibilityStatus
Issue-Key: wisdom|GetAIAgent|4xx->2xx|$
Issue-Key: wisdom|GetAIAgent|4xx->2xx|$.message
Issue-Key: wisdom|GetAIAgent|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: wisdom|GetAIAgent|4xx->2xx|status
Issue-Commit: eda2a37d7ec6811505bc428e1369649599383d34

# QConnect GetAIAgent read path ignores AI Agent lifecycle mutations and serves the create-time snapshot

> _Note: this issue was identified against the code base at commit `eda2a37d7ec6811505bc428e1369649599383d34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAIAgent

## Description
Reading back an AI Agent in the Amazon Q in Connect emulator (wire service `wisdom`, model com.amazonaws.qconnect) does not describe the agent's current state: the read is served from the record as it was created, so neither a mutation nor a deletion performed after creation is ever visible through GetAIAgent. TWO causes, both on the GetAIAgent read path, both must be closed.

Cause 1 of 2 - GetAIAgent serves the create-time agent snapshot and an aiAgentArn with no version qualifier (fixture qconnect-ai-agent-lifecycle, step task#5).
AWS: Smithy: GetAIAgent (@readonly, GET /assistants/{assistantId}/aiagents/{aiAgentId}, 200) returns `aiAgent` -> AIAgentData, the same shape UpdateAIAgent (POST /assistants/{assistantId}/aiagents/{aiAgentId}, output aiAgent -> AIAgentData) returns; AIAgentData declares required `visibilityStatus` (VisibilityStatus), required `aiAgentArn` (Arn) and optional `description` (Description, @length 1..255, pattern ^[a-zA-Z0-9\s_.,-]+). UpdateAIAgent takes a required `visibilityStatus` and an optional `description`, so after a publish/update the next read reports the UPDATED values. For the ARN the model says nothing about a qualifier (recorded behavior only): the recorded 200 shows AWS returning the AI-Agent ARN with the version qualifier of the version being read appended, i.e. `arn:<partition>:wisdom:<region>:<account>:ai-agent/<assistantId>/<aiAgentId>:$LATEST` for the unversioned working copy. Fixed text is the `arn:...:wisdom:...:ai-agent/` prefix and the trailing `:$LATEST`; partition, region, account id, assistantId and aiAgentId are values from the request and the emulator's state, never literals of one run. Likewise `description` and `visibilityStatus` are whatever the preceding mutation set, not one recording's strings.
Emulator: answers 200 but with the agent exactly as created - `visibilityStatus` 'SAVED' where the recording reports 'PUBLISHED', the create-time `description` where the recording reports the updated one, and `aiAgentArn` with no version qualifier at all (`.../ai-agent/<assistantId>/<aiAgentId>`).
Why: a client that publishes an agent and reads it back to confirm sees an agent still in SAVED with its old description, concludes the publish never took effect, and either loops retrying or wires an assistant to an agent it believes is unpublished; a client that keys references, IAM resource matching or assistant aiAgentConfiguration off the ARN it read back builds an identifier AWS never issues, so round-tripping a stored ARN against a freshly read one never matches.
Fix: the AI-Agent state in the qconnect service crate plus the GetAIAgent handler/AIAgentData serializer that projects it - the stored agent record must hold visibilityStatus and description as last written, the read must return current stored values rather than a creation snapshot, and the ARN must be rendered with the version qualifier of the version being read. (Writing the mutated values is UpdateAIAgent's half; that operation is still a 501 scaffold with its own auto-filed stub issue, which does NOT close this read-side divergence.)

Cause 2 of 2 - GetAIAgent keeps answering 200 for an AI Agent that no longer exists, instead of 404 ResourceNotFoundException (fixture qconnect-ai-agent-lifecycle, step task#10).
AWS: Smithy: GetAIAgent models ResourceNotFoundException (errorKind client, HTTP 404, doc "The specified resource does not exist.", members `message` and `resourceName`); DeleteAIAgent (DELETE /assistants/{assistantId}/aiagents/{aiAgentId}, 204) removes the agent, after which a read of that aiAgentId under that assistant is that 404. The recorded response is HTTP 404 with errortype header `ResourceNotFoundException` and body `message` = "AI Agent does not exist." - that message is entirely fixed text for this error class (no request- or state-derived parts).
Emulator: after the agent is gone it still answers HTTP 200 with the full aiAgent body (same assistantId/aiAgentId/name/configuration as before), emits no x-amzn-errortype header and no `message` member; the fixture's poll for error code ResourceNotFoundException never converges (29 attempts).
Why: a client that deletes an agent and polls the read to confirm removal never observes removal, so teardown and reconciliation loops hang forever; worse, a deleted agent stays resolvable and can still be referenced by assistants and sessions, which on AWS would fail closed.
Fix: the GetAIAgent handler's lookup and the AI-Agent state in the qconnect service crate - a read must miss once the agent has been removed from the assistant's agent map and return the modeled ResourceNotFoundException (404) with that message, rather than resolving against a record the delete left behind. (DeleteAIAgent is itself a 501 scaffold covered by its own auto-filed stub issue; the read-side miss is filed here because the read must reflect whatever the stored state says.)

Related open issues, none of which cover these GetAIAgent causes: 20261030T082651Z-wisdom-qconnect-getaiguardrail-serves-the-create-time-guardrail-snapshot.md (same fault shape on the AI-Guardrail read path), 20261029T194637Z-wisdom-assistant-ai-agent-configuration-reads-return-fabricated.md and 20261030T100255Z-wisdom-default-answer-recommendation-aiagentid-carries-a-numeric.md (assistant/session aiAgentConfiguration).

Traces: qconnect-ai-agent-lifecycle#task#5 (GetAIAgent, expected 200 / actual 200, 3 diverging members); qconnect-ai-agent-lifecycle#task#10 (GetAIAgent, expected 404 / actual 200).

## Evidence
Replay of corpus simparity-wire-aws-v1 against the emulator (emulator_up true, bootstrap_signal false).

Cause 1 - step qconnect-ai-agent-lifecycle#task#5, GetAIAgent, verdict mismatch, expected_status 200, actual_status 200:
  $.aiAgent.aiAgentArn: expected the AI-Agent ARN ending in the ':$LATEST' version qualifier (recorded 'arn:aws:wisdom:us-west-2:000000000000:ai-agent/<assistantId>/<aiAgentId>:$LATEST'), got the same ARN with no qualifier.
  $.aiAgent.description: expected the description set by the preceding mutation (recorded 'Updated probe manual search agent'), got the create-time description ('Probe manual search agent').
  $.aiAgent.visibilityStatus: expected 'PUBLISHED', got 'SAVED'.

Cause 2 - step qconnect-ai-agent-lifecycle#task#10, GetAIAgent, verdict mismatch, expected_status 404, actual_status 200:
  status: expected 404, got 200.
  $headers.x-amzn-errortype: expected the error type header (ResourceNotFoundException), got none.
  $.message: expected 'AI Agent does not exist.', got None.
  poll: error code None not in ['ResourceNotFoundException'] after 29 attempt(s); the emulator kept returning the full aiAgent body (visibilityStatus 'SAVED', origin 'CUSTOMER', status 'ACTIVE').

Model checked: services/service-com-amazonaws-qconnect-wisdomservice/smithy-model/com-amazonaws-qconnect-wisdomservice.json - GetAIAgent (@readonly, GET /assistants/{assistantId}/aiagents/{aiAgentId}, 200, errors incl. ResourceNotFoundException httpError 404), UpdateAIAgent (POST same URI, required visibilityStatus, optional description), DeleteAIAgent (DELETE same URI, 204), AIAgentData (aiAgentArn/visibilityStatus required, description optional), ResourceNotFoundException (client, 404, members message/resourceName).
Code: GetAIAgent is implemented at services/service-com-amazonaws-qconnect-wisdomservice/src/ops/get_ai_agent.rs (its header already claims an unknown aiAgentId is the modelled 404). UpdateAIAgent and DeleteAIAgent are still scaffolds returning Scaffolded::not_implemented (services/.../src/ops/update_ai_agent.rs, services/.../src/ops/delete_ai_agent.rs:162) and carry their own auto-filed stub issues.

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 fixture qconnect-ai-agent-lifecycle against the emulator.
2. The fixture creates an assistant, creates a MANUAL_SEARCH AI Agent under it, then mutates the agent (publishing it and replacing its description) and at step task#5 issues GET /assistants/{assistantId}/aiagents/{aiAgentId}. Compare the body against the recording: the emulator answers 200 with visibilityStatus 'SAVED', the create-time description, and an aiAgentArn with no ':$LATEST' qualifier.
3. The fixture then deletes the AI Agent and at step task#10 polls GET /assistants/{assistantId}/aiagents/{aiAgentId} for the modeled 404. The emulator answers 200 with the full aiAgent body on every attempt, with no x-amzn-errortype header and no 'AI Agent does not exist.' message, so the poll never converges.
