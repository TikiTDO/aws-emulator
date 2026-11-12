Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent.delete-knowledge-base-documents-proves-gone-and-unusable
Issue-Shape-Id: com.amazonaws.bedrockagent#AmazonBedrockAgentBuildTimeLambda
Issue-Key: bedrock-agent.delete-knowledge-base-documents-proves-gone-and-unusable|run#15|Timeout|time.sleep(5)
Issue-Commit: 6bbc9158b8c554ea5f9f7bb4834b16ee710ed040

# bedrock-agent: a document whose delete has completed is still reported by ListKnowledgeBaseDocuments, so DeleteKnowledgeBaseDocuments never becomes observable (1 cause, 1 check)

> _Note: this issue was identified against the code base at commit `6bbc9158b8c554ea5f9f7bb4834b16ee710ed040`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListKnowledgeBaseDocuments

## Description
Cause 1 of 1: a CUSTOM knowledge-base document that DeleteKnowledgeBaseDocuments has carried all the way through its delete chain is never dropped from the data source's document listing, so ListKnowledgeBaseDocuments keeps enumerating it indefinitely and no client can ever observe the delete having taken effect at the destination.

AWS: DeleteKnowledgeBaseDocuments (POST /knowledgebases/{knowledgeBaseId}/datasources/{dataSourceId}/documents/deleteDocuments, HTTP 202, @idempotent) removes the documents it names from the data source. ListKnowledgeBaseDocuments (POST /knowledgebases/{knowledgeBaseId}/datasources/{dataSourceId}/documents, HTTP 200, paginated over documentDetails) enumerates the documents the data source holds; once a delete has run to completion the deleted identifier stops appearing in that listing, while every other document of the same data source is untouched. Smithy: KnowledgeBaseDocumentDetail$status documents the delete chain as "DELETING – You submitted the delete job containing the document", "DELETE_IN_PROGRESS – The document is being deleted" and "NOT_FOUND – The document wasn't found" — NOT_FOUND is the status of a document the service holds nothing for, i.e. the answer a targeted GetKnowledgeBaseDocuments gives for an identifier that is gone, not a steady-state entry of the data source's own listing. The delete is also idempotent (clientToken is an @idempotencyToken), and the freed identifier is re-ingestible.

Emulator: the deleted identifier never leaves the listing. Replay of bedrock-agent.delete-knowledge-base-documents-proves-gone-and-unusable exhausts its whole budget ("Check timed out after 60.0s") inside the loop that re-reads ListKnowledgeBaseDocuments after the delete and waits for the deleted identifier to disappear; it never does, so the check never reaches the scoping, idempotence and identifier-reuse assertions that follow. In the store, a document's delete chain only rewrites the record's status — the record itself is kept forever (services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/src/state.rs, materialise_due_transitions walks document.pending_status to DocumentStatus::NotFound but never removes the entry from `documents`, unlike the data-source/knowledge-base arms right below it which do remove their records once removal_due_at passes), and the listing projects every record of the pair regardless of status (same file, page_of_documents filters only on knowledge_base_id/data_source_id). So a deleted document is served as a listing entry whose status is NOT_FOUND for the lifetime of the data source.

Why: a client that deletes a document and then confirms the removal through the data source's own listing — the normal way to confirm, since the delete is asynchronous and its own 202 response only echoes the accepted identifiers — never converges: the identifier stays enumerated, so reconciliation loops, document-count checks and "is this document gone?" gates hang or conclude the delete failed. The listing's documentDetails is also polluted with entries for documents that no longer exist, which skews any count or pagination built on it, and the status NOT_FOUND appearing as a listed document contradicts the member's own meaning.

Fix: belongs in the bedrock-agent document state and its listing projection — the clock's document transition chain must retire the record when the delete reaches its terminal state (the same way an accepted knowledge-base/data-source removal is applied), and/or page_of_documents must not project records whose delete has completed. Targeted reads (GetKnowledgeBaseDocuments, DeleteKnowledgeBaseDocuments) should keep answering such an identifier with the model's NOT_FOUND detail, which is already what the not-found projection produces for an identifier nothing is stored for.

Traces: bedrock-agent.delete-knowledge-base-documents-proves-gone-and-unusable. Note this check was NOT marked ALREADY FILED/OWNED by the suite: it times out before it ever reaches a stubbed call, and the open issue 20261105T115813Z-bedrock-agent-deleteknowledgebasedocuments-stub.md does not cover it — DeleteKnowledgeBaseDocuments is implemented in this checkout (services/.../src/ops/delete_knowledge_base_documents.rs starts the delete chain); what is missing is the retirement of the record at the end of that chain.

## Evidence
Expected (AWS): after DeleteKnowledgeBaseDocuments on a CUSTOM document, a subsequent ListKnowledgeBaseDocuments for the same knowledgeBaseId/dataSourceId stops reporting that document's identifier (its twin, ingested identically, is still reported and still INDEXED); a repeat delete of the dead identifier is accepted; re-ingesting the identifier works and it is listed again.

Actual (emulator): the deleted identifier is still present in documentDetails on every subsequent ListKnowledgeBaseDocuments call. The replayed check loops on that listing (its own 180s wait) and is killed by the suite budget — reported verdict: `[mismatch] bedrock-agent.delete-knowledge-base-documents-proves-gone-and-unusable`, detail `Error: Check timed out after 60.0s`. Had the loop ever been allowed to end, the check's own message states the shape of the divergence: "document <doomed-id> was still listed 180s after DeleteKnowledgeBaseDocuments; ListKnowledgeBaseDocuments returned [<doomed-id>, <survivor-id>]" (the ids are per-run values; the requirement is that only the surviving identifier remains).

Code grounding (state, not a patch): services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/src/state.rs — document transitions at the `for document in self.documents.values_mut()` arm of materialise_due_transitions mutate status to NotFound and clear pending_status but never erase the map entry; page_of_documents collects `self.documents.values()` filtered only by knowledge_base_id/data_source_id ("Every document of the data source is reported, whatever its ingestion status").

## Steps to Reproduce
1. Create an S3 Vectors-backed VECTOR knowledge base and wait for ACTIVE, then CreateDataSource with dataSourceConfiguration {type: CUSTOM} and wait for AVAILABLE.
2. IngestKnowledgeBaseDocuments twice (two distinct CUSTOM in-line documents, e.g. ids A and B) and poll GetKnowledgeBaseDocuments until both report status INDEXED.
3. ListKnowledgeBaseDocuments(knowledgeBaseId, dataSourceId) — both A and B are listed (correct).
4. DeleteKnowledgeBaseDocuments(knowledgeBaseId, dataSourceId, documentIdentifiers=[{dataSourceType: CUSTOM, custom: {id: A}}]) — accepted with 202.
5. Poll ListKnowledgeBaseDocuments. Expected: A stops being listed within seconds, leaving exactly [B]. Observed: A remains in documentDetails indefinitely (its status settles at NOT_FOUND but it is still an enumerated document), so the poll never converges and the check is killed at the 60s suite budget.
