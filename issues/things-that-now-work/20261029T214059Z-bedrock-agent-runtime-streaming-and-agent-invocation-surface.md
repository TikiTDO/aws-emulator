Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agent-runtime-agent-invoke-and-memory-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agent-runtime-agent-invoke-and-memory-errors bedrock-agent-runtime-invoke-agent-nonexistent bedrock-agent-runtime-agentic-retrieve-stream-invalid-foundation-model-type bedrock-agent-runtime-agentic-retrieve-stream-nonexistent-kb bedrock-agent-runtime-agentic-retrieve-stream-not-found bedrock-agent-runtime-flow-execution-not-found bedrock-agent-runtime-invoke-flow-variants-unknown-flow bedrock-agent-runtime-invoke-agent-invalid-session-id bedrock-agent-runtime-knowledge-base-retrieve-not-found bedrock-agent-runtime-retrieve-and-generate-sync-and-stream bedrock-agent-runtime-malformed-flow-identifier-validation-errors bedrock-agent-runtime-optimize-prompt-for-target-model bedrock-agent-runtime-optimize-prompt-workflow
Issue-Shape-Id: com.amazonaws.bedrockagentruntime#AmazonBedrockAgentRunTimeService
Issue-Key: bedrock-agent-runtime|InvokeAgent|4xx->4xx|$.message
Issue-Key: bedrock-agent-runtime|InvokeAgent|4xx->4xx|$headers.content-type
Issue-Key: bedrock-agent-runtime|InvokeAgent|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agent-runtime|AgenticRetrieveStream|4xx->5xx|$.message
Issue-Key: bedrock-agent-runtime|AgenticRetrieveStream|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agent-runtime|AgenticRetrieveStream|4xx->5xx|status
Issue-Key: bedrock-agent-runtime|InvokeFlow|2xx->4xx|$.__wire_eventstream__
Issue-Key: bedrock-agent-runtime|InvokeFlow|2xx->4xx|$headers.content-type
Issue-Key: bedrock-agent-runtime|InvokeFlow|2xx->4xx|status
Issue-Key: bedrock-agent-runtime|InvokeAgent|4xx->4xx|status
Issue-Key: bedrock-agent-runtime|RetrieveAndGenerateStream|4xx->5xx|$.message
Issue-Key: bedrock-agent-runtime|RetrieveAndGenerateStream|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agent-runtime|RetrieveAndGenerateStream|4xx->5xx|status
Issue-Key: bedrock-agent-runtime|InvokeFlow|4xx->4xx|$.message
Issue-Key: bedrock-agent-runtime|InvokeFlow|4xx->4xx|$headers.content-type
Issue-Key: bedrock-agent-runtime|InvokeFlow|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agent-runtime|InvokeFlow|4xx->4xx|status
Issue-Key: bedrock-agent-runtime|OptimizePrompt|2xx->5xx|$.__wire_eventstream__
Issue-Key: bedrock-agent-runtime|OptimizePrompt|2xx->5xx|$headers.content-type
Issue-Key: bedrock-agent-runtime|OptimizePrompt|2xx->5xx|status
Issue-Key: bedrock-agent-runtime|OptimizePrompt|4xx->5xx|$.message
Issue-Key: bedrock-agent-runtime|OptimizePrompt|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agent-runtime|OptimizePrompt|4xx->5xx|status
Issue-Commit: 7240e32f2500d20910730c71d4ad0714ff1f3217

# bedrock-agent-runtime streaming and agent-invocation surface is unreachable: InvokeAgent/InvokeFlow unrouted and four streaming operations answer 501

> _Note: this issue was identified against the code base at commit `7240e32f2500d20910730c71d4ad0714ff1f3217`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeAgent

## Description
NINE net-new root causes, all reached through replayed bedrock-agent-runtime wire fixtures. All nine share one fault class: the operations whose output is an HTTP-payload event stream are not served at all — three of them are not even routed into the bedrock-agent-runtime handler (the front door answers 'no emulated service is registered for bedrock'), four answer 501 NotImplemented, and the one path that IS routed delivers its error as an AWS-JSON HTTP error instead of inside the event stream. Smithy model consulted: services/service-com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice/smithy-model/com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice.json. Not covered by the open issue 20261023T004328Z-bedrock-agent-runtime-retrieve-optimizeprompt-getagentmemory-deleteagentmemory-and.md (that issue is a behavioral finding about Retrieve/GetAgentMemory/DeleteAgentMemory/GetDocumentContent and does not carry the replay key of any step below), nor by 20261029T171944Z-bedrock-agent-runtime-ten-net-new-causes-session.md (session/invocation message wording and CloudFormation), nor by 20261029T171944Z-bedrock-agent-runtime-retrieve-stub.md (Retrieve only).

=== Cause 1 of 9: InvokeAgent is not routed at all, so a nonexistent agent yields an UnknownService 404 instead of ResourceNotFoundException ===
AWS: Smithy: InvokeAgent is POST /agents/{agentId}/agentAliases/{agentAliasId}/sessions/{sessionId}/text, output member `completion` -> ResponseStream @httpPayload @streaming, and it models ResourceNotFoundException @httpError 404. Recorded behavior: invoking an agent id/alias that does not exist is refused HTTP 404 ResourceNotFoundException, Content-Type `application/json`, header `x-amzn-errortype: ResourceNotFoundException:http://internal.amazon.com/coral/com.amazon.bedrock.agent.runtimeservice/`, and Message `Failed to retrieve resource because it doesn't exist. Retry the request with a different resource identifier.` That message is ENTIRELY fixed text — no identifier, no member name, no variable part at all.
Emulator: HTTP 404 with Content-Type `application/x-amz-json-1.1`, `x-amzn-errortype: UnknownService`, and Message `no emulated service is registered for 'bedrock'` — i.e. the request never reaches a bedrock-agent-runtime handler; the endpoint-to-service router does not recognise the /agents/... route for this service at all.
Why: the whole agent-invocation surface is unreachable. A client gets an emulator-internal routing diagnostic with the wrong error type, the wrong Content-Type and a message that identifies no AWS error class, so neither SDK error unmarshalling nor retry/branching on ResourceNotFoundException works.
Fix: register the bedrock-agent-runtime /agents/{agentId}/agentAliases/{agentAliasId}/sessions/{sessionId}/text route in the emulator's endpoint/service router, then give InvokeAgent a handler whose resource-resolution branch raises the fixed-text ResourceNotFoundException with the bedrock.agent.runtimeservice errortype and `application/json`.
Traces: bedrock-agent-runtime-agent-invoke-and-memory-errors#task#1 (also bedrock-agent-runtime-invoke-agent-nonexistent#task#1).

=== Cause 2 of 9: AgenticRetrieveStream answers 501 NotImplemented instead of validating its enum members ===
AWS: Smithy: AgenticRetrieveStream is POST /agenticRetrieveStream with required messages/retrievers/agenticRetrieveConfiguration, output `stream` @httpPayload @streaming, and it models ValidationException @httpError 400. Recorded behavior: an `agenticRetrieveConfiguration.foundationModelType` outside the modeled enum is refused 400 ValidationException with `x-amzn-errortype: ValidationException:http://internal.amazon.com/coral/com.amazon.coral.validate/` and the standard Coral constraint message: fixed frame `1 validation error detected: Value '` / `' at '` / `' failed to satisfy constraint: Member must satisfy enum value set: [` / `]`; variable parts are the rejected value and the dotted member path, both from the request, and the bracketed list is the member's modeled enum values in model order (CUSTOM, MANAGED).
Emulator: 501 with `x-amzn-errortype: NotImplemented` and Message `operation POST /agenticRetrieveStream is not implemented in this emulator` — the operation has no handler, so no input validation runs.
Why: a client cannot exercise agentic retrieval at all, and a request AWS refuses with a 400 the SDK models is answered with an unmodeled 501 that no SDK error branch handles.
Fix: implement the AgenticRetrieveStream handler in the bedrock-agent-runtime crate, including input constraint validation for agenticRetrieveConfiguration's enum members before any work is done.
Traces: bedrock-agent-runtime-agentic-retrieve-stream-invalid-foundation-model-type#task#1.

=== Cause 3 of 9: AgenticRetrieveStream answers 501 instead of the knowledge-base ResourceNotFoundException ===
AWS: Smithy: AgenticRetrieveStream models ResourceNotFoundException @httpError 404. Recorded behavior: naming a knowledge base id that does not exist in `retrievers` is refused 404 ResourceNotFoundException with `x-amzn-errortype: ResourceNotFoundException:http://internal.amazon.com/coral/com.amazon.bedrock.agent.runtimeservice/` and Message `Knowledge Base with id <knowledgeBaseId> does not exist` — fixed text `Knowledge Base with id ` ... ` does not exist`, the single variable part being the knowledge-base id taken from the request, with no trailing period.
Emulator: 501 `NotImplemented`, `operation POST /agenticRetrieveStream is not implemented in this emulator`.
Why: the not-found path of agentic retrieval is unreachable, so a client cannot distinguish a missing knowledge base from an unimplemented service.
Fix: the AgenticRetrieveStream handler's retriever-resolution branch in the bedrock-agent-runtime crate.
Traces: bedrock-agent-runtime-agentic-retrieve-stream-nonexistent-kb#task#1 (also bedrock-agent-runtime-agentic-retrieve-stream-not-found#task#1).

=== Cause 4 of 9: InvokeFlow reports a missing flow as an HTTP error instead of delivering resourceNotFoundException inside the event stream ===
AWS: Smithy: InvokeFlow is POST /flows/{flowIdentifier}/aliases/{flowAliasIdentifier}, @http code 200, output member `responseStream` -> FlowResponseStream @httpPayload @streaming, and FlowResponseStream carries `resourceNotFoundException` as one of its event members. Recorded behavior: once the request is well formed, the response is HTTP 200 with Content-Type `application/vnd.amazon.eventstream` and the failure is delivered as an event-stream event of type `resourceNotFoundException` carrying a `message` member — the error travels in the stream, not in the HTTP status line.
Emulator: HTTP 404 with Content-Type `application/x-amz-json-1.1` and no event stream at all (`__wire_eventstream__` absent).
Why: an SDK that opens InvokeFlow's response stream and iterates events never sees the modeled exception event; it sees a transport-level 404 instead, so the documented in-stream error contract — the only way a flow reports a mid-execution failure — is unobservable. Code written against the emulator never handles the in-stream variant.
Fix: the InvokeFlow handler and its event-stream serializer in the bedrock-agent-runtime crate — once the operation is dispatched it must answer 200 `application/vnd.amazon.eventstream` and frame modeled exceptions as FlowResponseStream events.
Traces: bedrock-agent-runtime-flow-execution-not-found#task#1 (also bedrock-agent-runtime-invoke-flow-variants-unknown-flow#task#1 and #task#2).

=== Cause 5 of 9: InvokeAgent's sessionId @pattern is never validated, because the route is unrouted ===
AWS: Smithy: InvokeAgent.sessionId is an @httpLabel, required, SessionId @length(2,100) @pattern `^[0-9a-zA-Z._:-]+$`; InvokeAgent models ValidationException @httpError 400. Recorded behavior: a sessionId containing characters outside that class is refused 400 ValidationException, Content-Type `application/json`, `x-amzn-errortype: ValidationException:http://internal.amazon.com/coral/com.amazon.coral.validate/`, Message with the Coral frame `1 validation error detected: Value '` / `' at '` / `' failed to satisfy constraint: Member must satisfy regular expression pattern: ` followed by the pattern. AWS renders the pattern with the outer anchors STRIPPED: `[0-9a-zA-Z._:-]+`. Variable parts: the rejected value and the member name `sessionId`, both from the request.
Emulator: HTTP 404 `UnknownService` / `no emulated service is registered for 'bedrock'`, Content-Type `application/x-amz-json-1.1` — the malformed label is never validated because the route does not exist (same routing gap as Cause 1, surfaced on the validation path).
Why: a client sending a malformed session id gets a 404 instead of the modeled 400, so input-validation error handling inverts (not-found vs invalid-argument) and the SDK cannot tell the caller which member was wrong.
Fix: route registration plus InvokeAgent's httpLabel constraint validation, rendering the @pattern with outer anchors stripped.
Traces: bedrock-agent-runtime-invoke-agent-invalid-session-id#task#1.

=== Cause 6 of 9: RetrieveAndGenerateStream answers 501 instead of the knowledge-base ResourceNotFoundException ===
AWS: Smithy: RetrieveAndGenerateStream is POST /retrieveAndGenerateStream, output `stream` @httpPayload @streaming plus a required `sessionId` header, and it models ResourceNotFoundException @httpError 404. Recorded behavior: a retrieveAndGenerateConfiguration naming a knowledge base that does not exist is refused 404 ResourceNotFoundException with Message `Knowledge Base with id <knowledgeBaseId> does not exist` — same fixed text as Cause 3, the id being the only variable part.
Emulator: 501 with `x-amzn-errortype: NotImplemented` and `operation POST /retrieveAndGenerateStream is not implemented in this emulator`.
Why: the streaming RAG entry point is entirely unavailable; a client cannot stream a retrieve-and-generate answer, and the not-found refusal it would branch on is replaced by an unmodeled 501.
Fix: implement the RetrieveAndGenerateStream handler and its event-stream serializer in the bedrock-agent-runtime crate, including knowledge-base resolution.
Traces: bedrock-agent-runtime-knowledge-base-retrieve-not-found#task#3 (also bedrock-agent-runtime-retrieve-and-generate-sync-and-stream#task#2).

=== Cause 7 of 9: InvokeFlow's flowIdentifier @pattern is never validated, because the route is unrouted ===
AWS: Smithy: InvokeFlow.flowIdentifier is a required @httpLabel, FlowIdentifier @length(max 2048) @pattern `^(arn:aws:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:flow/[0-9a-zA-Z]{10})|([0-9a-zA-Z]{10})$`; InvokeFlow models ValidationException @httpError 400. Recorded behavior: a flowIdentifier that is neither a flow ARN nor a 10-character flow id is refused 400 ValidationException, Content-Type `application/json`, `x-amzn-errortype: ValidationException:http://internal.amazon.com/coral/com.amazon.coral.validate/`, with the Coral constraint frame quoting the pattern with the outer anchors stripped: `((arn:aws:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:flow/[0-9a-zA-Z]{10})|([0-9a-zA-Z]{10}))`. Variable parts: the rejected value and the member name `flowIdentifier`.
Emulator: HTTP 404 `UnknownService` / `no emulated service is registered for 'bedrock'`, Content-Type `application/x-amz-json-1.1`.
Why: same inversion as Cause 5 for the flow surface — an invalid-argument request is answered as a routing not-found.
Fix: register the /flows/{flowIdentifier}/aliases/{flowAliasIdentifier} route and validate the httpLabel members before dispatch.
Traces: bedrock-agent-runtime-malformed-flow-identifier-validation-errors#task#2.

=== Cause 8 of 9: OptimizePrompt answers 501 instead of the 200 event stream carrying analyzePromptEvent and optimizedPromptEvent ===
AWS: Smithy: OptimizePrompt is POST /optimize-prompt, @http code 200, output member `optimizedPrompt` -> OptimizedPromptStream @httpPayload @streaming. Recorded behavior: a well-formed OptimizePrompt answers HTTP 200 with Content-Type `application/vnd.amazon.eventstream` and emits, in order, an `analyzePromptEvent` event carrying a `message` member and an `optimizedPromptEvent` event carrying an `optimizedPrompt` member.
Emulator: 501 with Content-Type `application/json` and no event stream at all.
Why: prompt optimization is unavailable, and because the emulator answers `application/json` rather than an event stream, an SDK that opens the streaming response fails on the content type before it ever reaches an error branch.
Fix: implement the OptimizePrompt handler and the OptimizedPromptStream event-stream serializer in the bedrock-agent-runtime crate.
Traces: bedrock-agent-runtime-optimize-prompt-for-target-model#task#1 (also bedrock-agent-runtime-optimize-prompt-workflow#task#1).

=== Cause 9 of 9: OptimizePrompt answers 501 instead of refusing an invalid targetModelId with 400 ValidationException ===
AWS: Smithy: OptimizePrompt.targetModelId is required, @length(1,2048) with a model-identifier @pattern, and OptimizePrompt models ValidationException @httpError 400. Recorded behavior: a targetModelId that is not a resolvable model identifier is refused 400 ValidationException with `x-amzn-errortype: ValidationException:http://internal.amazon.com/coral/com.amazon.bedrock.agent.runtimeservice/` — the service's OWN errortype namespace, not the Coral validate one used for constraint violations — and Message `Input validation failed. Check your request parameters and retry the request. Invalid model ID: The provided model identifier is invalid.` That message is entirely fixed text; it echoes neither the rejected model id nor a member name.
Emulator: 501 `NotImplemented` / `operation POST /optimize-prompt is not implemented in this emulator`.
Why: the invalid-model refusal is both a different status and a different error class from what a client branches on, and the distinction AWS draws between its service-namespace ValidationException and the Coral constraint ValidationException is lost.
Fix: the OptimizePrompt handler's targetModelId resolution branch — raise the fixed-text ValidationException under the bedrock.agent.runtimeservice errortype namespace.
Traces: bedrock-agent-runtime-optimize-prompt-workflow#task#2.

## Evidence
Cause 1 — step bedrock-agent-runtime-agent-invoke-and-memory-errors#task#1 (cluster also covers bedrock-agent-runtime-invoke-agent-nonexistent#task#1): status 404 matched, but $.message expected "Failed to retrieve resource because it doesn't exist. Retry the request with a different resource identifier.", got "no emulated service is registered for 'bedrock'"; $headers.content-type expected 'application/json', got 'application/x-amz-json-1.1'; $headers.x-amzn-errortype expected 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazon.bedrock.agent.runtimeservice/', got 'UnknownService'.
Cause 2 — bedrock-agent-runtime-agentic-retrieve-stream-invalid-foundation-model-type#task#1: status expected 400, got 501; $.message expected "1 validation error detected: Value 'BOGUS_TYPE' at 'agenticRetrieveConfiguration.foundationModelType' failed to satisfy constraint: Member must satisfy enum value set: [CUSTOM, MANAGED]", got 'operation POST /agenticRetrieveStream is not implemented in this emulator'; x-amzn-errortype expected 'ValidationException:http://internal.amazon.com/coral/com.amazon.coral.validate/', got 'NotImplemented'. 'BOGUS_TYPE' is the per-run rejected value filling the template, not part of it.
Cause 3 — bedrock-agent-runtime-agentic-retrieve-stream-nonexistent-kb#task#1 (cluster also bedrock-agent-runtime-agentic-retrieve-stream-not-found#task#1): status expected 404, got 501; $.message expected 'Knowledge Base with id KBNOTFND1 does not exist', got the NotImplemented prose; x-amzn-errortype expected 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazon.bedrock.agent.runtimeservice/', got 'NotImplemented'. KBNOTFND1 is the per-run id.
Cause 4 — bedrock-agent-runtime-flow-execution-not-found#task#1 (cluster also bedrock-agent-runtime-invoke-flow-variants-unknown-flow#task#1 and #task#2): status expected 200, got 404; $.__wire_eventstream__ expected [{'type': 'resourceNotFoundException', 'keys': ['message']}], got None; content-type expected 'application/vnd.amazon.eventstream', got 'application/x-amz-json-1.1'.
Cause 5 — bedrock-agent-runtime-invoke-agent-invalid-session-id#task#1: status expected 400, got 404; $.message expected "1 validation error detected: Value 'bad session id!' at 'sessionId' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9a-zA-Z._:-]+", got "no emulated service is registered for 'bedrock'"; content-type expected 'application/json', got 'application/x-amz-json-1.1'; x-amzn-errortype expected 'ValidationException:http://internal.amazon.com/coral/com.amazon.coral.validate/', got 'UnknownService'.
Cause 6 — bedrock-agent-runtime-knowledge-base-retrieve-not-found#task#3 (cluster also bedrock-agent-runtime-retrieve-and-generate-sync-and-stream#task#2): status expected 404, got 501; $.message expected 'Knowledge Base with id KBBD995DA3 does not exist', got 'operation POST /retrieveAndGenerateStream is not implemented in this emulator'; x-amzn-errortype got 'NotImplemented'.
Cause 7 — bedrock-agent-runtime-malformed-flow-identifier-validation-errors#task#2: status expected 400, got 404; $.message expected "1 validation error detected: Value 'bad' at 'flowIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: ((arn:aws:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:flow/[0-9a-zA-Z]{10})|([0-9a-zA-Z]{10}))", got "no emulated service is registered for 'bedrock'"; content-type and x-amzn-errortype as in Cause 5.
Cause 8 — bedrock-agent-runtime-optimize-prompt-for-target-model#task#1 (cluster also bedrock-agent-runtime-optimize-prompt-workflow#task#1): status expected 200, got 501; $.__wire_eventstream__ expected [{'type': 'analyzePromptEvent', 'keys': ['message']}, {'type': 'optimizedPromptEvent', 'keys': ['optimizedPrompt']}], got None; content-type expected 'application/vnd.amazon.eventstream', got 'application/json'.
Cause 9 — bedrock-agent-runtime-optimize-prompt-workflow#task#2: status expected 400, got 501; $.message expected 'Input validation failed. Check your request parameters and retry the request. Invalid model ID: The provided model identifier is invalid.', got 'operation POST /optimize-prompt is not implemented in this emulator'; x-amzn-errortype expected 'ValidationException:http://internal.amazon.com/coral/com.amazon.bedrock.agent.runtimeservice/', got 'NotImplemented'.

## Steps to Reproduce
Replay the wire corpus simparity-wire-aws-v1 against the emulator; each cause is one step below.
Cause 1: POST /agents/<10-char agentId>/agentAliases/<10-char aliasId>/sessions/<sessionId>/text for an agent that was never created -> expect 404 ResourceNotFoundException with the fixed not-found text; emulator answers 404 UnknownService. (fixture bedrock-agent-runtime-agent-invoke-and-memory-errors, step task#1)
Cause 2: POST /agenticRetrieveStream with agenticRetrieveConfiguration.foundationModelType set to a value outside {CUSTOM, MANAGED} -> expect 400 ValidationException; emulator answers 501. (bedrock-agent-runtime-agentic-retrieve-stream-invalid-foundation-model-type#task#1)
Cause 3: POST /agenticRetrieveStream naming a knowledge-base id that does not exist -> expect 404 ResourceNotFoundException; emulator answers 501. (bedrock-agent-runtime-agentic-retrieve-stream-nonexistent-kb#task#1)
Cause 4: POST /flows/<flowIdentifier>/aliases/<flowAliasIdentifier> for a flow that does not exist -> expect 200 application/vnd.amazon.eventstream whose first event is resourceNotFoundException; emulator answers 404 application/x-amz-json-1.1. (bedrock-agent-runtime-flow-execution-not-found#task#1)
Cause 5: POST the InvokeAgent URI with a sessionId containing a space and a '!' -> expect 400 ValidationException naming 'sessionId'; emulator answers 404 UnknownService. (bedrock-agent-runtime-invoke-agent-invalid-session-id#task#1)
Cause 6: POST /retrieveAndGenerateStream naming a knowledge-base id that does not exist -> expect 404 ResourceNotFoundException; emulator answers 501. (bedrock-agent-runtime-knowledge-base-retrieve-not-found#task#3)
Cause 7: POST the InvokeFlow URI with flowIdentifier='bad' -> expect 400 ValidationException naming 'flowIdentifier'; emulator answers 404 UnknownService. (bedrock-agent-runtime-malformed-flow-identifier-validation-errors#task#2)
Cause 8: POST /optimize-prompt with a valid input prompt and targetModelId -> expect 200 event stream with analyzePromptEvent then optimizedPromptEvent; emulator answers 501 application/json. (bedrock-agent-runtime-optimize-prompt-for-target-model#task#1)
Cause 9: POST /optimize-prompt with an unresolvable targetModelId -> expect 400 ValidationException with the fixed 'Input validation failed... Invalid model ID' text; emulator answers 501. (bedrock-agent-runtime-optimize-prompt-workflow#task#2)
