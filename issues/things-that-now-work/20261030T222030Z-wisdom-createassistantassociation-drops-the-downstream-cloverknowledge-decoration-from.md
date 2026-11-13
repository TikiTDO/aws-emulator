Issue-Source: trace-sweep-divergence
Issue-Trace: wisdom-assistant-association-error-paths
Issue-Step: task#2
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wisdom-assistant-association-error-paths
Issue-Shape-Id: com.amazonaws.wisdom#WisdomService
Issue-Key: wisdom|CreateAssistantAssociation|4xx->4xx|$.message
Issue-Commit: 5cbf701d3f2858b74a0bd70e66429a88601fcbf9

# Wisdom CreateAssistantAssociation drops the downstream CloverKnowledge decoration from its ResourceNotFoundException message

> _Note: this issue was identified against the code base at commit `5cbf701d3f2858b74a0bd70e66429a88601fcbf9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAssistantAssociation

## Description
1 cause, in the wisdom service.

Cause 1 of 1: CreateAssistantAssociation knowledge-base-not-found message text.
AWS: POST /assistants/{assistantId}/associations with associationType KNOWLEDGE_BASE and an association.knowledgeBaseId that does not resolve answers 404 ResourceNotFoundException (Smithy: CreateAssistantAssociation errors -> ResourceNotFoundException, @error client, @httpError 404, doc 'The specified resource does not exist.'; members message -> String, resourceName -> String). Wisdom resolves the knowledge base through its internal knowledge-base service and surfaces that downstream failure verbatim in the body's `message`, so the recorded AWS message is the fixed sentence 'Knowledge Base does not exist.' FOLLOWED BY the downstream decoration ' (Service: CloverKnowledge, Status Code: 404, Request ID: <uuid>) (SDK Attempt Count: 1)'. Fixed text of that template: 'Knowledge Base does not exist.', the literals ' (Service: CloverKnowledge, Status Code: ', ', Request ID: ', ') (SDK Attempt Count: ' and the trailing ')'. Variable parts: the status code (404, the status of the downstream not-found) and the attempt count (1 on a first, unretried attempt); the request ID is a per-run identifier minted by the emulator for that downstream call and is NOT a fixed value — the recorded 'dca9f128-0310-4528-820c-183541af93ba' fills the template, it is not part of it. Recorded behavior only for the decoration itself: the Smithy model constrains neither `message` nor its text, so the shape of the message comes from the recording; Hypothesis: the decoration is the AWS Java SDK's rendering of the internal CloverKnowledge exception, leaked into Wisdom's own error body.
Emulator: same 404 and the same x-amzn-errortype ResourceNotFoundException, and the leading sentence is right, but the body is only 'Knowledge Base does not exist.' — the whole downstream decoration (service name, downstream status code, request ID, attempt count) is absent.
Why: a client that parses the message to recover which downstream component failed, or that logs/forwards the embedded request ID when opening a support case, gets nothing from the emulator while the status and error type look correct; message-equality assertions on this error class fail, and tooling that regex-extracts 'Request ID: ...' from the message finds no match and either errors or records an empty correlation id.
Fix: the ResourceNotFoundException construction in the wisdom CreateAssistantAssociation handler's knowledge-base resolution path — it needs to render the downstream failure (service name, downstream status, the request id it minted for the internal call, attempt count) into the message instead of emitting only the bare sentence. The status mapping and errortype header are already correct. This is a distinct site from the two message literals already filed as open issue 20261029T192454Z-wisdom-error-message-text-diverges-from-aws-wording.md (GetQuickResponse and NotifyRecommendationsReceived), which does not cover CreateAssistantAssociation and whose causes are plain fixed-text misspellings rather than a missing templated suffix.
Traces: wisdom-assistant-association-error-paths#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, score 72.5%, 10 divergence clusters).

wisdom-assistant-association-error-paths#task#2, CreateAssistantAssociation, expected 404 / actual 404:
  $.message: value_mismatch
    expected 'Knowledge Base does not exist. (Service: CloverKnowledge, Status Code: 404, Request ID: dca9f128-0310-4528-820c-183541af93ba) (SDK Attempt Count: 1)'
    got      'Knowledge Base does not exist.'

The HTTP status (404), the x-amzn-errortype header (ResourceNotFoundException) and every other body member matched; the only divergence is the missing decoration suffix on the message. Within that suffix the request-ID value is per-run and is not itself the requirement — the requirement is that the suffix with its fixed literals and its service/status/attempt-count values be present at all.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Create an assistant, then POST /assistants/{assistantId}/associations with associationType=KNOWLEDGE_BASE and association.knowledgeBaseId set to a knowledge-base id that does not exist.
3. Observe the 404 body: `message` is exactly 'Knowledge Base does not exist.' with no ' (Service: CloverKnowledge, Status Code: 404, Request ID: <uuid>) (SDK Attempt Count: 1)' suffix, where AWS appends that downstream decoration.
4. Fixture wisdom-assistant-association-error-paths, step task#2.
