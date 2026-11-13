Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent.knowledge-base-document-blob-round-trip
Issue-Key: bedrock-agent|Retrieve|NotImplemented
Issue-Commit: f392f82803b1854878505550560cb55ee4e9c61d

# bedrock-agent: Retrieve answers 501 NotImplemented, so an ingested CUSTOM knowledge-base document can never be read back (1 cause, 1 check)

> _Note: this issue was identified against the code base at commit `f392f82803b1854878505550560cb55ee4e9c61d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Retrieve

## Description
Cause 1 of 1: the bedrock-agent knowledge-base document round-trip reaches its final, load-bearing step — reading the ingested document back through the bedrock-agent-runtime data plane — and that operation is still a scaffold stub, so the whole ingest/index/retrieve contract is unverifiable from the bedrock-agent side. Failing check ids (1): bedrock-agent.knowledge-base-document-blob-round-trip.

AWS: Smithy (com.amazonaws.bedrockagentruntime#Retrieve): http {method: POST, uri: /knowledgebases/{knowledgeBaseId}/retrieve, code: 200}; input knowledgeBaseId is an @httpLabel, @required, KnowledgeBaseIdentifier @length 10..2048 @pattern ^[0-9a-zA-Z]{10}$|^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:knowledge-base/[0-9a-zA-Z]{10}$; retrievalQuery (KnowledgeBaseQuery) is @required and @sensitive; optional retrievalConfiguration, guardrailConfiguration, userContext, nextToken. Output retrievalResults (KnowledgeBaseRetrievalResults) is @required, plus optional guardrailAction (INTERVENED|NONE) and nextToken; the operation is @readonly and @paginated (inputToken/outputToken nextToken, items retrievalResults). Modeled errors only: AccessDeniedException 403, BadGatewayException 502, ConflictException 409, DependencyFailedException 424, InternalServerException 500, ResourceNotFoundException 404, ServiceQuotaExceededException 400, ThrottlingException 429, ValidationException 400. There is no NotImplemented in the model and no 501 in its error set: a well-formed Retrieve against a knowledge base that holds an INDEXED document answers 200 with retrievalResults whose content.text carries the document's text byte-for-byte, including multi-byte UTF-8 (the check ingests 'café münchen 日本語 emoji 🎉' via IngestKnowledgeBaseDocuments with content.custom.inlineContent.byteContent and expects it back unchanged).

Emulator: the request is refused with HTTP 501 and error code NotImplemented, message 'operation Retrieve is scaffolded but not implemented in this emulator'. Fixed text of that message is 'operation ' ... ' is scaffolded but not implemented in this emulator'; its only variable part is the operation name ('Retrieve'), taken from the handler, not from the request or any state. Everything before it in the check succeeds: CreateKnowledgeBase, CreateDataSource(type=CUSTOM), IngestKnowledgeBaseDocuments (which correctly echoes documentDetails[0].identifier.custom.id) and GetKnowledgeBaseDocuments reaching INDEXED all answer. Only the read-back is refused.

Why: a client can ingest into a knowledge base and watch a document reach INDEXED, then has no way to read it back — the only operation that proves ingestion actually stored anything answers a status and an error code AWS never returns for this API. 501/NotImplemented is not in Retrieve's modeled error set, so SDK retry and error-handling paths do not classify it: botocore raises a bare ClientError with code NotImplemented instead of one of the nine modeled exceptions, and any RAG application built against the emulator fails at its first query with an error it cannot interpret. Byte-fidelity of stored unicode content is also untestable.

Fix: the Retrieve operation's handler. It must look up the knowledge base by knowledgeBaseId (ARN or 10-char id form), search the documents ingested into its data sources, and return 200 with retrievalResults populated from the stored content — refusing with the modeled ResourceNotFoundException (404) for an unknown knowledge base and ValidationException (400) for a bad request, never a 501. Related open work that does NOT cover this check (the suite marked it neither ALREADY FILED nor OWNED): 20261029T171944Z-bedrock-agent-runtime-retrieve-stub.md and 20261025T173258Z-bedrock-agent-runtime-retrieve-is-a-scaffold-stub.md both carry the same stub under the bedrock-agent-runtime / bedrock-agent-runtime__s3 service tokens; this check is the bedrock-agent-side ingest round trip and is not on either issue's replay key.

Traces: bedrock-agent.knowledge-base-document-blob-round-trip (fails at _poll_for_marker_in_retrieval -> runtime.retrieve). Stub marker and 501 live in services/service-com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice/src/ops/retrieve.rs lines 4-6 and 194.

## Evidence
Expected (AWS): after CreateKnowledgeBase + CreateDataSource(dataSourceConfiguration.type=CUSTOM) + IngestKnowledgeBaseDocuments with documents[0].content.custom = {customDocumentIdentifier:{id:'sim2real-blob-doc-1'}, sourceType:'IN_LINE', inlineContent:{type:'BYTE', byteContent:{mimeType:'text/plain', data:<utf-8 bytes>}}}, and GetKnowledgeBaseDocuments reporting status INDEXED, a POST /knowledgebases/{knowledgeBaseId}/retrieve with retrievalQuery.text answers HTTP 200 and a body whose retrievalResults[*].content.text contains the ingested text byte-identically (multi-byte UTF-8 preserved).

Actual (emulator): HTTP 501, error code NotImplemented, message 'operation Retrieve is scaffolded but not implemented in this emulator'. botocore surfaces it as:
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the Retrieve operation: operation Retrieve is scaffolded but not implemented in this emulator
The status 501 and the code NotImplemented are both outside Retrieve's modeled error set (403/502/409/424/500/404/400/429/400 with AccessDeniedException, BadGatewayException, ConflictException, DependencyFailedException, InternalServerException, ResourceNotFoundException, ServiceQuotaExceededException, ThrottlingException, ValidationException). Generated knowledge-base ids, data-source ids and request ids differ per run and are not part of the requirement.

## Steps to Reproduce
1. bedrock-agent CreateKnowledgeBase for a vector store, poll GetKnowledgeBase until status ACTIVE.
2. bedrock-agent CreateDataSource(knowledgeBaseId=<kb>, name=<name>, dataSourceConfiguration={'type':'CUSTOM'}); poll GetDataSource until AVAILABLE.
3. bedrock-agent IngestKnowledgeBaseDocuments(knowledgeBaseId=<kb>, dataSourceId=<ds>, documents=[{'content':{'dataSourceType':'CUSTOM','custom':{'customDocumentIdentifier':{'id':'sim2real-blob-doc-1'},'sourceType':'IN_LINE','inlineContent':{'type':'BYTE','byteContent':{'mimeType':'text/plain','data':'S2R-MARKER-7f3a: café münchen 日本語 emoji 🎉 blob-roundtrip'.encode()}}}}}]).
4. Poll bedrock-agent GetKnowledgeBaseDocuments until documentDetails[0].status == INDEXED (this succeeds on the emulator).
5. bedrock-agent-runtime Retrieve(knowledgeBaseId=<kb>, retrievalQuery={'text':'blob-roundtrip'}).
6. Observe HTTP 501 / NotImplemented 'operation Retrieve is scaffolded but not implemented in this emulator' instead of HTTP 200 with retrievalResults containing the ingested unicode text.
