Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent.update-data-source-chunking-configuration-change-rejected
Issue-Shape-Id: com.amazonaws.bedrockagent#AmazonBedrockAgentBuildTimeLambda
Issue-Key: bedrock-agent.update-data-source-chunking-configuration-change-rejected|_refusal#17|AssertionError|raise AssertionError(f"expected {what} to be refused, but it succeeded")
Issue-Commit: 441f41d603a1aa5ce812e262390a74b2fc08e0c7

# bedrock-agent: UpdateDataSource accepts a chunkingConfiguration change instead of refusing it with ValidationException (immutability not enforced)

> _Note: this issue was identified against the code base at commit `441f41d603a1aa5ce812e262390a74b2fc08e0c7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDataSource

## Description
Cause 1 of 1: UpdateDataSource does not enforce the immutability of vectorIngestionConfiguration.chunkingConfiguration. Failing check: bedrock-agent.update-data-source-chunking-configuration-change-rejected.

AWS: UpdateDataSource (Smithy: PUT /knowledgebases/{knowledgeBaseId}/datasources/{dataSourceId}, input UpdateDataSourceRequest with required name + dataSourceConfiguration and optional vectorIngestionConfiguration; modeled errors include ValidationException @httpError 400). The model marks the member optional and says nothing further about mutability, so the rule is recorded behavior only: as measured against real AWS (us-west-2, 2026-09-17), the chunkingConfiguration of a data source connector is fixed at creation. UpdateDataSource is full-replace for vectorIngestionConfiguration, not a partial patch, so AWS refuses with HTTP 400 ValidationException whenever the EFFECTIVE chunkingConfiguration would diverge from what is stored — both (a) when a different, otherwise perfectly valid chunkingConfiguration is sent explicitly (e.g. FIXED_SIZE maxTokens/overlapPercentage changed from the stored values), and (b) when vectorIngestionConfiguration is omitted entirely, since the omission defaults chunking to something other than the stored value. The recorded message is fixed text with no variable parts: "vectorIngestionConfiguration.chunkingConfiguration cannot be updated once created." The refusal is total: no part of the update is applied — neither the chunking nor the other mutable members in the same request (description/name). An UpdateDataSource that explicitly re-sends the ORIGINAL chunkingConfiguration while changing other members succeeds (HTTP 200), which is the positive control.

Emulator: the positive-control update succeeds, but the immutability check is simply absent. The replay sends the identical UpdateDataSource with only chunkingConfiguration changed (FIXED_SIZE with different maxTokens/overlapPercentage than the stored config) and the emulator answers 200 and applies the change, producing no error at all: `AssertionError: expected UpdateDataSource changing chunkingConfiguration after creation to be refused, but it succeeded`. Expected status 400 with error code ValidationException and message "vectorIngestionConfiguration.chunkingConfiguration cannot be updated once created."; actual: HTTP 200 success response. Because the check aborts on the first missing refusal, the omission leg (vectorIngestionConfiguration absent) was never reached, but it is the same missing validation and the same required 400/ValidationException/message, so a fix must cover both the explicit-change and the field-omitted forms, and must leave the stored chunking AND the other members (description) untouched on refusal.

Why: a client that relies on AWS's behavior gets silent data corruption of its knowledge base. Code that re-sends a connector definition it believes is unchanged (or that omits vectorIngestionConfiguration, expecting a patch) is refused by AWS but silently mutates the chunking in the emulator; the vectors already embedded under the original chunking no longer match the connector's declared chunking, and the application never sees the error path AWS makes it handle. Conversely, error-handling code and infrastructure tooling that expects to recover from this ValidationException is never exercised, so the emulator passes deployments that real AWS rejects.

Fix: belongs in the UpdateDataSource handler's validation/state-reconciliation step for the bedrock-agent build-time service (services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda, ops/update_data_source.rs): before mutating stored state, resolve the effective vectorIngestionConfiguration.chunkingConfiguration for the request (treating an absent vectorIngestionConfiguration as full-replace/default, not as a no-op), compare it to the stored connector's chunkingConfiguration, and on any divergence return ValidationException (HTTP 400) with the fixed message above and apply NO part of the update (atomic refusal, so description/name stay at their stored values).

Traces: bedrock-agent.update-data-source-chunking-configuration-change-rejected (mismatch, assertion at beat 2 of the check). Related open issue 20261029T172025Z-bedrock-agent-data-source-projection-drops-connector-defaults.md concerns data-source PROJECTION defaults on create/read and does not cover update-time immutability of chunkingConfiguration.

## Evidence
Check: bedrock-agent.update-data-source-chunking-configuration-change-rejected [mismatch]

Setup (succeeds): CreateDataSource with type S3 and vectorIngestionConfiguration.chunkingConfiguration = {chunkingStrategy: FIXED_SIZE, fixedSizeChunkingConfiguration: {maxTokens: 300, overlapPercentage: 20}}; GetDataSource confirms that chunking is stored; a positive-control UpdateDataSource re-sending the SAME chunking with a new description succeeds and the new description is readable.

Diverging leg — UpdateDataSource with only chunkingConfiguration changed to {chunkingStrategy: FIXED_SIZE, fixedSizeChunkingConfiguration: {maxTokens: 500, overlapPercentage: 10}}:
  Expected (real AWS, measured): HTTP 400, Error.Code = "ValidationException", Error.Message = "vectorIngestionConfiguration.chunkingConfiguration cannot be updated once created." (entirely fixed text — no request- or state-derived substitutions), and GetDataSource afterwards still shows chunking maxTokens 300 / overlapPercentage 20 AND the pre-rejection description (nothing partially applied).
  Actual (emulator): HTTP 200 success, update applied, no error raised.
  Harness output: File "check_bedrock_agent_update_data_source_chunking_configuration_change_rejected.py", line 126, in run / line 258, in _refusal: AssertionError: expected UpdateDataSource changing chunkingConfiguration after creation to be refused, but it succeeded

Not reached because of the abort, same root cause and same required response: UpdateDataSource omitting vectorIngestionConfiguration entirely must also yield HTTP 400 / ValidationException / the same message (full-replace semantics), leaving stored chunking and description untouched.

Smithy (services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda): UpdateDataSource PUT /knowledgebases/{knowledgeBaseId}/datasources/{dataSourceId}; vectorIngestionConfiguration -> VectorIngestionConfiguration (optional); errors include ValidationException (client, httpError 400), ConflictException (409). Observed AWS classification is ValidationException, not ConflictException.

## Steps to Reproduce
1. Create an S3-Vectors-backed knowledge base and a data source connector on it:
   aws bedrock-agent create-data-source --knowledge-base-id <KB> --name <ds-name> --description 'original description' --data-source-configuration '{"type":"S3","s3Configuration":{"bucketArn":"<bucket-arn>"}}' --vector-ingestion-configuration '{"chunkingConfiguration":{"chunkingStrategy":"FIXED_SIZE","fixedSizeChunkingConfiguration":{"maxTokens":300,"overlapPercentage":20}}}'
2. Wait for the data source to reach AVAILABLE; confirm via get-data-source that the stored chunkingConfiguration is FIXED_SIZE 300/20.
3. Positive control — update with the SAME chunking and a new description; it must succeed and the description must change.
4. Diverging call — repeat the update changing ONLY the chunking:
   aws bedrock-agent update-data-source --knowledge-base-id <KB> --data-source-id <DS> --name <ds-name> --description 'this description must never be stored' --data-source-configuration '{"type":"S3","s3Configuration":{"bucketArn":"<bucket-arn>"}}' --vector-ingestion-configuration '{"chunkingConfiguration":{"chunkingStrategy":"FIXED_SIZE","fixedSizeChunkingConfiguration":{"maxTokens":500,"overlapPercentage":10}}}'
   Emulator: HTTP 200 and the change is applied. AWS: HTTP 400 ValidationException 'vectorIngestionConfiguration.chunkingConfiguration cannot be updated once created.'
5. Variant that must behave identically — repeat the update with --vector-ingestion-configuration omitted entirely; AWS refuses it with the same 400/ValidationException/message (full-replace, not a patch).
6. After each refusal, get-data-source must still report chunking 300/20 and the description from step 3 (no partial application).
