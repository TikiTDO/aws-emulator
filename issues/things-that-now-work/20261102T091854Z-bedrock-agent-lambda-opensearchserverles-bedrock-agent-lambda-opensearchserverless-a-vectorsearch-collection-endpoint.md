Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent__lambda__opensearchserverless.bedrock-kb-opensearchserverless-vector-index-creation-positive
Issue-Key: bedrock-agent__lambda__opensearchserverless.bedrock-kb-opensearchserverless-vector-index-creation-positive|_create_vector_index#14|AssertionError|raise AssertionError(f"could not create vector index after retries: {last_exc}")
Issue-Commit: 486151182441fa9779198fc02e1d6d3c56236874

# bedrock-agent__lambda__opensearchserverless: a VECTORSEARCH collection endpoint refuses the OpenSearch REST index create with 400 SerializationException "missing required header `x-amz-target`", so no vector index (and no knowledge base) can be provisioned (1 cause)

> _Note: this issue was identified against the code base at commit `486151182441fa9779198fc02e1d6d3c56236874`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIndex

## Description
ONE root cause, reached through the cross-service bedrock-agent + lambda + opensearchserverless check (service token taken from the leading segment of the check id). Cause count: 1 — this single cause must be closed.

=== Cause 1 of 1: the published collectionEndpoint of an ACTIVE VECTORSEARCH collection serves the AWS-JSON RPC front door instead of the OpenSearch REST data plane ===
AWS: recorded behavior only — a collection's data plane is the OpenSearch REST API and is not described by the opensearchserverless Smithy model (which covers only the control plane: CreateCollection/BatchGetCollection/CreateSecurityPolicy/CreateAccessPolicy). Recorded behavior: once BatchGetCollection reports a VECTORSEARCH collection ACTIVE and publishes collectionEndpoint, a SigV4-signed request with service name `aoss` of the form `PUT https://<collectionEndpoint>/<indexName>` carrying a knn_vector mapping body — exactly what opensearch-py's indices.create issues, with NO x-amz-target header, because the OpenSearch REST API has no RPC target — is accepted (2xx, {"acknowledged": true}) when the collection's data-access policy names the calling principal, and refused 403 when it does not. That accepted index is the precondition for bedrock-agent CreateKnowledgeBase's OPENSEARCH_SERVERLESS storageConfiguration (vectorIndexName + fieldMapping must match the existing index) and for the ingestion job that follows.
Emulator: every such request is answered HTTP 400 with body {"__type":"SerializationException","message":"missing required header `x-amz-target`"}. The collection endpoint host falls through to the emulator's AWS-JSON protocol dispatcher, which requires an X-Amz-Target header, so no OpenSearch REST verb (index create/get/delete, document index, search) is servable on a collection endpoint for ANY principal.
Why: a client that follows AWS's documented knowledge-base flow can never create the vector index, so CreateKnowledgeBase's precondition is unreachable and no ingestion job or Retrieve/RetrieveAndGenerate call can be exercised. It also destroys the data-access-policy boundary: a permitted principal and an unpermitted one both get the identical 400 SerializationException, so 'allowed' and 'denied' are indistinguishable and a protocol error is reported where AWS reports success or 403.
Fix: the opensearchserverless collection data plane plus the emulator's endpoint/host routing — resolve the published collectionEndpoint host to an OpenSearch REST handler (index create/get/delete, document index/search) that authorises SigV4 `aoss` requests against the collection's data-access policy, rather than letting the host reach the AWS-JSON x-amz-target dispatcher (services/service-com-amazonaws-opensearchserverless-opensearchserverless and the host/endpoint routing table that publishes collection endpoints).
NOTE for the fixer: the OPEN issue 20261029T171944Z-bedrock-agent-lambda-opensearchserverles-bedrock-agent-lambda-opensearchserverless-2-net-new-causes.md describes this same 400 as its cause 1, and 20261029T191222Z-bedrock-agent-lambda-opensearchserverles-...-4-net-new-causes.md describes the same front-door wiring. The check below was NOT marked owned by either (its replay key differs — it fails in its own retry wrapper with 'could not create vector index after retries'), so it is recorded here to be re-replayed; if those issues' fix lands first, this cause should close with it rather than be fixed twice.
Traces: bedrock-agent__lambda__opensearchserverless.bedrock-kb-opensearchserverless-vector-index-creation-positive.

## Evidence
Expected: the SigV4(`aoss`)-signed `PUT https://<collectionEndpoint>/<indexName>` with the knn_vector mapping is accepted for the principal the data-access policy names, so the vector index exists and bedrock-agent CreateKnowledgeBase's vectorIndexName/fieldMapping precondition is satisfied.
Actual (bedrock-agent__lambda__opensearchserverless.bedrock-kb-opensearchserverless-vector-index-creation-positive): AssertionError: could not create vector index after retries: RequestError(400, '{"__type":"SerializationException","message":"missing required header `x-amz-target`"}') — raised from _create_vector_index after its retry loop. The status 400, the __type 'SerializationException' and that message are fixed emulator output, independent of the collection name, index name, principal or endpoint host (all per-run values).

## Steps to Reproduce
1. iam.create_role for the knowledge-base role; note its ARN.
2. opensearchserverless.create_security_policy(type='encryption') and (type='network', AllowFromPublic) for collection <c>.
3. opensearchserverless.create_access_policy(type='data', Principal=[<kb role arn>, <caller arn>], Rules granting aoss:* on collection/<c> and index/<c>/*).
4. opensearchserverless.create_collection(name=<c>, type='VECTORSEARCH'); poll batch_get_collection until status ACTIVE and read collectionEndpoint.
5. Sign with SigV4, service name 'aoss', and issue `PUT https://<collectionEndpoint>/<indexName>` with a body mapping a knn_vector field (dimension 1536) plus text and metadata fields — e.g. opensearch-py OpenSearch(...).indices.create(index=<indexName>, body=<mapping>).
6. Expect 2xx {'acknowledged': true}; the emulator answers HTTP 400 {"__type":"SerializationException","message":"missing required header `x-amz-target`"} on every retry, and the same 400 for a principal the data-access policy omits (where AWS answers 403).
