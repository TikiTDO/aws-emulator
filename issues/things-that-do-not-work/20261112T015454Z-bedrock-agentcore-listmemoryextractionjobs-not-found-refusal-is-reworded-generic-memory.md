Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-memory-extraction-ingest-not-found-errors
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-memory-extraction-ingest-not-found-errors
Issue-Shape-Id: com.amazonaws.bedrockagentcore#AmazonBedrockAgentCore
Issue-Key: bedrock-agentcore|ListMemoryExtractionJobs|4xx->4xx|$.message
Issue-Commit: 6bbc9158b8c554ea5f9f7bb4834b16ee710ed040

# ListMemoryExtractionJobs not-found refusal is reworded: generic "Memory not found: <id>" instead of AWS's fixed MemoryItem sentence

> _Note: this issue was identified against the code base at commit `6bbc9158b8c554ea5f9f7bb4834b16ee710ed040`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListMemoryExtractionJobs

## Description
Cause 1 of 1: ListMemoryExtractionJobs refuses a listing addressed at a non-existent memory container with the right status and error class, but with a message this emulator authored rather than the sentence AWS produces for a missing memory container.

AWS: Smithy (services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/smithy-model/com-amazonaws-bedrockagentcore-amazonbedrockagentcore.json) models ListMemoryExtractionJobs as POST /memories/{memoryId}/extractionJobs with @http code 200, memoryId a @required @httpLabel (MemoryId, @length min 12, @pattern admitting either the bare id or the memory ARN), and ResourceNotFoundException with @httpError 404 among its errors ("The exception that occurs when the specified resource does not exist. This can happen when using an invalid identifier or when trying to access a resource that has been deleted."). The model does not state the message text, so: recorded behavior only — for a well-formed memoryId naming no existing container the call is refused 404 ResourceNotFoundException with the message "MemoryItem does not exist". That message is ENTIRELY fixed text: it has no variable part, and in particular it does not name the memoryId from the request path.

Emulator: answers the correct 404 ResourceNotFoundException but with the message "Memory not found: <memoryId>". The refusal arm at services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/src/ops/list_memory_extraction_jobs.rs:204-216 propagates whatever message the shared existence gate crate::state::memory_container_resolved produces, which is the generic "<Label> not found: <identifier>" template built from NotFoundResource::Memory's label "Memory" (src/state.rs:1896-1906 and the message builder that follows at src/state.rs:1908+). So the lead-in "Memory not found: " is this emulator's own fixed template and the trailing token is the memoryId interpolated from the request path — a parameterization AWS does not perform for this refusal.

Why: a client or SDK conformance test that recognizes a missing memory container by matching AWS's fixed sentence "MemoryItem does not exist" (equality or substring) no longer matches, so this refusal cannot be classified by message as it can against AWS; and because the emulator interpolates the request's memoryId, the text logged on this path varies per request where AWS's does not.

Fix: the not-found arm of this operation's handler (services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/src/ops/list_memory_extraction_jobs.rs, the map_err over crate::state::memory_container_resolved) must emit AWS's fixed sentence for this refusal rather than passing through the generic resource_not_found_message template — that template is correct for the readers that DO name their resource, but is not AWS's wording here. Keep the 404 and the ResourceNotFoundException class, which already match.

Related but not covering: open issue 20261108T080658Z-bedrock-agentcore-batchcreatememoryrecords-not-found-refusal-reworded-emulator-authored-sentence.md records the same rewording for BatchCreateMemoryRecords, and 20261102T104223Z-bedrock-agentcore-memory-data-plane-refusal-messages-still-diverge-getevent.md covers GetEvent/GetMemoryRecord/DeleteMemoryRecord wording. Neither carries this operation or this step's replay key, and the replay reports this cluster as unowned, so it must be replayed on its own.

Traces: bedrock-agentcore-memory-extraction-ingest-not-found-errors#task#2.

## Evidence
From the wire replay of simparity-wire-aws-v1 against the booted emulator (emulator_up=true, bootstrap_signal=false, score_pct=50.0).

Fixture bedrock-agentcore-memory-extraction-ingest-not-found-errors, step task#2, ListMemoryExtractionJobs [mismatch], status 404 expected and 404 actual (status and x-amzn-errortype agree; only the message diverges):
  $.message: expected 'MemoryItem does not exist', got 'Memory not found: sim2real-probe-fakememory-abcdefghij'

Fixed vs variable text: the expected message is 100% fixed text ('MemoryItem does not exist') with NO variable part. In the actual message, 'Memory not found: ' is the emulator's own fixed lead-in (NotFoundResource::Memory's label plus the shared ' not found: ' template) and the trailing token is the memoryId taken from the request path — the id shown above is this run's request value and is not part of any requirement.

Source of the divergence (not a request-specific value): services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/src/ops/list_memory_extraction_jobs.rs:204-216 propagating the message from crate::state::memory_container_resolved, whose text comes from NotFoundResource::Memory (label at src/state.rs:1898) through the generic '<Label> not found: <identifier>' message builder in src/state.rs.

## Steps to Reproduce
1. Build and boot the emulator from this checkout, then replay the wire corpus simparity-wire-aws-v1.
2. POST /memories/<well-formed memoryId that no memory container exists for>/extractionJobs (fixture bedrock-agentcore-memory-extraction-ingest-not-found-errors, step task#2).
3. The response is 404 ResourceNotFoundException, as AWS answers, but its message reads 'Memory not found: <the memoryId from the path>' instead of AWS's fixed 'MemoryItem does not exist'.
4. Equivalent replay: re-run fixture bedrock-agentcore-memory-extraction-ingest-not-found-errors, step task#2.
