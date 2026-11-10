Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent-runtime.readonly-retrieve-cannot-mutate-kb-state bedrock-agent-runtime.get-agent-memory-nonexistent-agent-not-found bedrock-agent-runtime.delete-agent-memory-without-permission-blocked bedrock-agent-runtime.get-document-content-without-kb-access-denied bedrock-agent-runtime.optimize-prompt-returns-nonempty-result bedrock-agent-runtime.optimize-prompt-round-trip bedrock-agent-runtime.optimize-prompt-invalid-model-reference-validation-exception bedrock-agent-runtime.invoke-inline-agent-return-control-result-reenters-the-next-turn-prompt bedrock-agent-runtime.invoke-inline-agent-stream-final-response-flag-changes-chunk-framing bedrock-agent-runtime.flow-execution-events-ordering bedrock-agent-runtime.flow-execution-lifecycle-start-run-stop bedrock-agent-runtime.flow-execution-terminal-state-immutable-rejected bedrock-agent-runtime.get-flow-execution-nonexistent-not-found bedrock-agent-runtime.invoke-flow-stream-ends-on-a-single-completion-event-and-gates-traces bedrock-agent-runtime.stop-flow-execution-concurrent-race bedrock-agent-runtime.flow-execution-snapshot-availability-window bedrock-agent-runtime.get-document-content-nonexistent-document-not-found bedrock-agent-runtime.retrieve-and-generate-citation-spans-index-the-generated-text bedrock-agent-runtime.retrieve-number-of-results-truncates-one-stable-ranking bedrock-agent-runtime.knowledge-base-retrieval-scoring-deterministic bedrock-agent-runtime.retrieve-nonexistent-knowledge-base-not-found
Issue-Key: bedrock-agent-runtime.readonly-retrieve-cannot-mutate-kb-state|run#6|AssertionError|assert codes == ["ResourceNotFoundException"] * 3, (
Issue-Key: bedrock-agent-runtime.get-agent-memory-nonexistent-agent-not-found|run#3|AssertionError|assert result == "ResourceNotFoundException", (
Issue-Key: bedrock-agent-runtime.delete-agent-memory-without-permission-blocked|run#10|AssertionError|assert denied_code == "AccessDeniedException", denied_code
Issue-Key: bedrock-agent-runtime.get-document-content-without-kb-access-denied|run#10|AssertionError|assert denied_code == "AccessDeniedException", denied_code
Issue-Key: bedrock-agent-runtime.optimize-prompt-returns-nonempty-result|run#7|AssertionError|assert resp is not None, f"no candidate target model was usable in this account: {last_err}"
Issue-Key: bedrock-agent-runtime.optimize-prompt-round-trip|_optimize_with_any_candidate#3|AssertionError|raise AssertionError(f"no candidate target model was usable in this account: {last_err}")
Issue-Key: bedrock-agent-runtime.optimize-prompt-invalid-model-reference-validation-exception|run#5|AssertionError|assert events is not None, f"no candidate target model was usable in this account: {last_err}"
Issue-Key: bedrock-agent-runtime|InvokeInlineAgent|ValidationException
Issue-Key: bedrock-agent-runtime|CreateFlow|NotImplemented
Issue-Key: bedrock-agent-runtime.flow-execution-snapshot-availability-window|_create_passthrough_flow#5|Timeout|time.sleep(3)
Issue-Key: bedrock-agent-runtime|CreateKnowledgeBase|NotImplemented
Issue-Key: bedrock-agent-runtime|CreateAccessPolicy|NotImplemented
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# bedrock-agent-runtime: Retrieve, OptimizePrompt, GetAgentMemory, DeleteAgentMemory and GetDocumentContent answer NotImplemented, InvokeInlineAgent validates a member the operation does not have, and the flow/knowledge-base provisioning it needs is stubbed (9 causes)

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Retrieve

## Description
Nine distinct root causes, all reached through bedrock-agent-runtime checks. Smithy model consulted: services/service-com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice/smithy-model/com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice.json (list_operations confirms all 35 operations below are modeled and bound to the service).

=== Cause 1 of 9: Retrieve returns NotImplemented instead of the modeled ResourceNotFoundException for a knowledge base that never existed ===
AWS: Smithy: Retrieve is bound to the service and models ResourceNotFoundException (404) among its errors. Recorded behavior: three identical Retrieve calls against a never-provisioned knowledgeBaseId each answer ResourceNotFoundException, and the call is read-only so it never creates state.
Emulator: all three calls answer error code 'NotImplemented' (501) — the operation is a scaffold stub, so neither the not-found branch nor the read-only guarantee exists.
Why: a client cannot distinguish 'knowledge base does not exist' from 'service broken', and no retrieval path can be exercised at all.
Fix: implement the Retrieve handler (services/service-com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice/src/ops/retrieve.rs), including the ResourceNotFoundException branch for an unknown knowledgeBaseId.
Traces: bedrock-agent-runtime.readonly-retrieve-cannot-mutate-kb-state.

=== Cause 2 of 9: GetAgentMemory returns NotImplemented instead of ResourceNotFoundException ===
AWS: Smithy: GetAgentMemory is bound to the service and models ResourceNotFoundException (404). Recorded behavior: GetAgentMemory for an agentId/agentAliasId pair that was never created answers ResourceNotFoundException.
Emulator: error code 'NotImplemented'.
Why: the modeled not-found contract for agent memory is unreachable; a client's existence probe gets a 501.
Fix: implement the GetAgentMemory handler in the same crate's src/ops/get_agent_memory.rs.
Traces: bedrock-agent-runtime.get-agent-memory-nonexistent-agent-not-found.

=== Cause 3 of 9: DeleteAgentMemory returns NotImplemented instead of AccessDeniedException for an unauthorized caller ===
AWS: Smithy: DeleteAgentMemory is bound to the service and models AccessDeniedException (403) and ResourceNotFoundException (404). Recorded behavior: a principal without the delete permission is refused with AccessDeniedException, while a fully permitted caller making the identical call reaches the service and gets ResourceNotFoundException — which is what proves the first answer was authorization and not a blanket refusal.
Emulator: error code 'NotImplemented' for both callers, so the authorization boundary cannot be observed.
Fix: implement the DeleteAgentMemory handler (src/ops/delete_agent_memory.rs) with its authorization check ahead of the resource lookup.
Traces: bedrock-agent-runtime.delete-agent-memory-without-permission-blocked.

=== Cause 4 of 9: GetDocumentContent returns NotImplemented instead of AccessDeniedException / ResourceNotFoundException ===
AWS: Smithy: GetDocumentContent is bound to the service and models AccessDeniedException (403) and ResourceNotFoundException (404). Recorded behavior: an unpermitted caller gets AccessDeniedException; a permitted caller asking for the same never-ingested document gets ResourceNotFoundException.
Emulator: error code 'NotImplemented' on both paths.
Fix: implement the GetDocumentContent handler (src/ops/get_document_content.rs) with the authorization check and the not-found branch.
Traces: bedrock-agent-runtime.get-document-content-without-kb-access-denied.

=== Cause 5 of 9: OptimizePrompt has no route at all — POST /optimize-prompt answers NotImplemented ===
AWS: Smithy: OptimizePrompt is bound to the service and models ValidationException (400) for a malformed/unknown targetModelId as well as a streaming success response. Recorded behavior: a valid OptimizePrompt returns a non-empty optimized prompt; an invalid model reference is refused with ValidationException.
Emulator: 'An error occurred (NotImplemented) when calling the OptimizePrompt operation: operation POST /optimize-prompt is not implemented in this emulator' — note the message is route-level ('operation POST /optimize-prompt'), not the usual scaffolded-stub wording, so the URI is not even wired into the router.
Why: no prompt-optimization client works, and the negative class (ValidationException on a bad model reference) is indistinguishable from the missing route.
Fix: bind POST /optimize-prompt in the service router and implement the handler (src/ops/optimize_prompt.rs), including the ValidationException branch for an invalid targetModelId.
Traces: bedrock-agent-runtime.optimize-prompt-returns-nonempty-result, bedrock-agent-runtime.optimize-prompt-round-trip, bedrock-agent-runtime.optimize-prompt-invalid-model-reference-validation-exception.

=== Cause 6 of 9: InvokeInlineAgent validates an 'agentId' member that is not part of its input, rejecting every valid sessionId ===
AWS: Smithy: InvokeInlineAgent is POST /agents/{sessionId}; its input has NO agentId member. The @httpLabel member is sessionId -> SessionId, required, @length(2,100), @pattern ^[0-9a-zA-Z._:-]+$. AWS accepts a 20-character alphanumeric session id and answers 200 with the streaming completion (AccessDeniedException is the only per-account model-entitlement refusal).
Emulator: answers ValidationException (400) with "1 validation error detected: Value 's2rrcsessdbee5f0b2692' at 'agentId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-zA-Z]{10}$" — the value supplied is the sessionId, and ^[0-9a-zA-Z]{10}$ is the InvokeAgent agentId constraint. The POST /agents/{sessionId} route is being dispatched into (or validated by) the InvokeAgent input shape, so the path label is bound to the wrong member.
Why: InvokeInlineAgent is unusable for every legal sessionId longer than 10 characters or containing '.', '_', ':' or '-' — i.e. nearly all of them — and the 400 masquerades as a client error on a request AWS accepts. Hypothesis: a shared route table maps /agents/{...} to one handler.
Fix: route POST /agents/{sessionId} to the InvokeInlineAgent handler and validate the label against SessionId's own constraints (src/ops/invoke_inline_agent.rs and the router's path-label binding).
Traces: bedrock-agent-runtime.invoke-inline-agent-return-control-result-reenters-the-next-turn-prompt, bedrock-agent-runtime.invoke-inline-agent-stream-final-response-flag-changes-chunk-framing.

=== Cause 7 of 9: bedrock-agent CreateFlow is a scaffold stub, so the entire flow-execution runtime plane is unreachable ===
AWS: Smithy: the bedrock-agent-runtime flow plane (StartFlowExecution, GetFlowExecution, ListFlowExecutions, ListFlowExecutionEvents, StopFlowExecution, GetExecutionFlowSnapshot, InvokeFlow) operates on a flow created by the bedrock-agent control plane's CreateFlow; recorded behavior is that CreateFlow returns the flow and the runtime then drives it to Completed/Stopped, with ResourceNotFoundException (404) for an executionIdentifier that never existed and a terminal execution refusing further transitions.
Emulator: 'An error occurred (NotImplemented) when calling the CreateFlow operation: operation CreateFlow is scaffolded but not implemented in this emulator', so every runtime flow check dies in setup. One of them (flow-execution-snapshot-availability-window) does not even surface the error — it reports 'Check timed out after 60.0s' because its provisioning loop polls instead of failing fast.
Why: no flow can be created, so none of the runtime flow operations can be reached, and the event-ordering / terminal-state / stop-race invariants are all untested. A sibling stub in the same crate (GetFlow) already has its own filed issue; CreateFlow does not.
Fix: implement CreateFlow in services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/src/ops/create_flow.rs (with PrepareFlow/CreateFlowVersion/CreateFlowAlias as the checks' setup uses them) so the runtime flow operations have state to run against.
Traces: bedrock-agent-runtime.flow-execution-events-ordering, bedrock-agent-runtime.flow-execution-lifecycle-start-run-stop, bedrock-agent-runtime.flow-execution-terminal-state-immutable-rejected, bedrock-agent-runtime.get-flow-execution-nonexistent-not-found, bedrock-agent-runtime.invoke-flow-stream-ends-on-a-single-completion-event-and-gates-traces, bedrock-agent-runtime.stop-flow-execution-concurrent-race, bedrock-agent-runtime.flow-execution-snapshot-availability-window.

=== Cause 8 of 9: bedrock-agent CreateKnowledgeBase is a scaffold stub, so retrieval and citation behavior cannot be reached ===
AWS: recorded behavior only for the setup step: CreateKnowledgeBase returns a knowledgeBaseId that bedrock-agent-runtime's Retrieve / RetrieveAndGenerate / GetDocumentContent then operate on, with citation spans indexing the generated text and numberOfResults truncating a stable ranking.
Emulator: 'An error occurred (NotImplemented) when calling the CreateKnowledgeBase operation: operation CreateKnowledgeBase is scaffolded but not implemented in this emulator'.
Why: every knowledge-base-backed runtime behavior is untestable; a client cannot stand up the resource its retrieval calls require.
Fix: implement CreateKnowledgeBase in services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/src/ops/create_knowledge_base.rs.
Traces: bedrock-agent-runtime.get-document-content-nonexistent-document-not-found, bedrock-agent-runtime.retrieve-and-generate-citation-spans-index-the-generated-text, bedrock-agent-runtime.retrieve-number-of-results-truncates-one-stable-ranking.

=== Cause 9 of 9: opensearchserverless CreateAccessPolicy is a scaffold stub, blocking the vector-store setup a knowledge base needs ===
AWS: recorded behavior only: a knowledge base backed by an OpenSearch Serverless collection requires a data-access policy created with CreateAccessPolicy before ingestion; it returns the created policy detail.
Emulator: 'An error occurred (NotImplemented) when calling the CreateAccessPolicy operation: operation CreateAccessPolicy is scaffolded but not implemented in this emulator'.
Why: the deterministic-scoring and not-found retrieval checks cannot provision their vector store, so retrieval ranking fidelity is unverified.
Fix: implement CreateAccessPolicy in services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/ops/create_access_policy.rs.
Traces: bedrock-agent-runtime.knowledge-base-retrieval-scoring-deterministic, bedrock-agent-runtime.retrieve-nonexistent-knowledge-base-not-found.

## Evidence
Cause 1 — expected: ResourceNotFoundException on all 3 identical Retrieve calls against never-provisioned knowledge base 'SIM2REAK0B'. actual: ['NotImplemented', 'NotImplemented', 'NotImplemented'].
Cause 2 — expected: GetAgentMemory on a never-created agent/alias raises ResourceNotFoundException. actual: 'NotImplemented'.
Cause 3 — expected denied_code == 'AccessDeniedException'. actual: AssertionError: NotImplemented.
Cause 4 — expected denied_code == 'AccessDeniedException'. actual: AssertionError: NotImplemented.
Cause 5 — expected: 200 with a non-empty optimized prompt (and ValidationException for an invalid target model). actual: 'An error occurred (NotImplemented) when calling the OptimizePrompt operation: operation POST /optimize-prompt is not implemented in this emulator'.
Cause 6 — expected: 200 streaming completion for InvokeInlineAgent with sessionId 's2rrcsessdbee5f0b2692' (Smithy: sessionId @length(2,100) @pattern ^[0-9a-zA-Z._:-]+$; there is no agentId member). actual: 400 ValidationException "1 validation error detected: Value 's2rrcsessdbee5f0b2692' at 'agentId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-zA-Z]{10}$" (also seen as 's2rsfroffb17d1d1df5f3').
Cause 7 — expected: CreateFlow returns a flow id. actual: 501 'operation CreateFlow is scaffolded but not implemented in this emulator'; flow-execution-snapshot-availability-window instead reports 'Check timed out after 60.0s'.
Cause 8 — expected: CreateKnowledgeBase returns a knowledgeBaseId. actual: 501 'operation CreateKnowledgeBase is scaffolded but not implemented in this emulator'.
Cause 9 — expected: CreateAccessPolicy returns the created access policy. actual: 501 'operation CreateAccessPolicy is scaffolded but not implemented in this emulator'.

## Steps to Reproduce
Cause 1: bedrock-agent-runtime.retrieve(knowledgeBaseId='SIM2REAK0B', retrievalQuery={'text':'x'}) -> 501 NotImplemented instead of 404 ResourceNotFoundException.
Cause 2: bedrock-agent-runtime.get_agent_memory(agentId='ABCDEFGHIJ', agentAliasId='ZYXWVUTSRQ', memoryType='SESSION_SUMMARY', memoryId='x') -> 501.
Cause 3: mint STS credentials with no bedrock permissions and call delete_agent_memory(agentId='ABCDEFGHIJ', agentAliasId='ZYXWVUTSRQ') -> 501 instead of 403.
Cause 4: same locked-down client, get_document_content(knowledgeBaseId='ABCDEFGHIJ', dataSourceId='DATASRC01', documentIdentifiers=[...]) -> 501 instead of 403.
Cause 5: bedrock-agent-runtime.optimize_prompt(input={'textPrompt':{'text':'...'}}, targetModelId=<any>) -> 501 'operation POST /optimize-prompt is not implemented'.
Cause 6: bedrock-agent-runtime.invoke_inline_agent(sessionId='s2rrcsessdbee5f0b2692', foundationModel=<model id>, instruction=<>=40 chars>) -> 400 ValidationException naming 'agentId'.
Cause 7: bedrock-agent.create_flow(name=..., executionRoleArn=..., definition={passthrough Input->Output}) -> 501.
Cause 8: bedrock-agent.create_knowledge_base(...) -> 501.
Cause 9: opensearchserverless.create_access_policy(name=..., type='data', policy=...) -> 501.
