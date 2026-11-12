Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearchserverless.collection-endpoint-requires-aoss-sigv4-service-name opensearchserverless.data-access-policy-read-permission-without-write-blocks-indexing
Issue-Shape-Id: com.amazonaws.opensearchserverless#OpenSearchServerless
Issue-Key: opensearchserverless.collection-endpoint-requires-aoss-sigv4-service-name|run#20|AssertionError|assert status == 403, (
Issue-Key: opensearchserverless.data-access-policy-read-permission-without-write-blocks-indexing|run#32|Timeout|time.sleep(3.0)
Issue-Commit: ffea503ad0eca557448179da41e60128c8e8e2ec

# opensearchserverless: 2 causes on the collection data plane — an 'es'-scoped SigV4 request is ACCEPTED 201 instead of refused with the aoss front-door 403, and a data access policy's per-operation Permission list is still not evaluated

> _Note: this issue was identified against the code base at commit `ffea503ad0eca557448179da41e60128c8e8e2ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAccessPolicy

## Description
TWO root causes on the OpenSearch Serverless collection DATA plane (the host published as CollectionDetail.collectionEndpoint), grounded in two failing behavioral checks listed under the `opensearchserverless` suite heading.

Overlap note (named, not a duplicate claim): open issue 20261108T223024Z-opensearchserverless-3-net-new-causes-on-the-collection.md lists both of these check ids in its Issue-Trace-Ids, but the suite marked NEITHER check OWNED in this replay, and for Cause 1 the observed divergence has CHANGED class (that issue recorded a 403 carrying the wrong body; the refusal is now absent altogether), so both are filed here. If that issue is reworked to add credential-scope validation and data-access-policy evaluation to the data plane, these close with it.

Shared model context: the aoss data plane itself is not Smithy-modeled. Smithy (com.amazonaws.opensearchserverless) documents CollectionDetail.collectionEndpoint as the 'Collection-specific endpoint used to submit index, search, and data upload requests to an OpenSearch Serverless collection', and models the control-plane writers that configure its authorization: CreateSecurityPolicy(type: SecurityPolicyType, name, policy) and UpdateAccessPolicy(type: AccessPolicyType, name, policyVersion, policy) — both REPLACE the policy document; errors ConflictException/409, ResourceNotFoundException/404, ValidationException/400. The wire behavior of the endpoint is therefore recorded behavior only, as noted per cause.

=== Cause 1 of 2: the data plane does not validate the SigV4 credential scope, so a request signed for a different service is served ===
Failing check: opensearchserverless.collection-endpoint-requires-aoss-sigv4-service-name
AWS: recorded behavior only (the data plane is unmodeled); the signing requirement is documented in the OpenSearch Serverless developer guide — data-plane requests to a collection endpoint must be SigV4-signed with the signing service name `aoss`, and the principal must additionally hold the IAM permission `aoss:APIAccessAll`. Recorded: a request that is byte-for-byte identical to an ACCEPTED `aoss`-signed write (same method, URL, body, credentials, clock, and x-amz-content-sha256 payload digest) except that the credential scope names `es` is refused at the front door with HTTP 403 and the OpenSearch error document `{"status": 403, "error": {"type": "forbidden", "reason": "403 Forbidden"}}`. The 403 status, the fixed `error.type` value `forbidden` and the fixed `error.reason` text `403 Forbidden` are the requirement (nothing in that body derives from the request); `forbidden` is the discriminator that separates the front-door rejection from OpenSearch's own permission layer, which always names itself `security_exception`.
Emulator: the es-scoped write SUCCEEDS. The endpoint answers HTTP 201 with a real OpenSearch write receipt — `{'_index': <index>, '_id': <id>, '_version': 1, 'result': 'created', '_shards': {'total': 1, 'successful': 1, 'failed': 0}, '_seq_no': <n>, '_primary_term': 1}` — i.e. the document is really created under a credential scoped to another service. The credential scope is not inspected at all. (In the earlier replay recorded by the issue named above the emulator at least refused 403, with the wrong body; the refusal is now gone, so this is strictly more permissive than before.)
Why: SigV4 scope binding is the mechanism that stops a credential or presigned request minted for one service from being replayed against another. A client (or a security test) that relies on an aoss endpoint rejecting non-aoss-scoped signatures gets no rejection, and code that branches on the 403/`forbidden` front-door answer to re-sign with the correct scope never takes that branch — it silently writes data instead.
Fix: the collection data-plane request handler's SigV4 verification step in the opensearchserverless service crate — it must parse the Authorization header's Credential scope and refuse any request whose service component is not `aoss` with 403 and the OpenSearch error document above, before the request reaches the index/document handlers.

=== Cause 2 of 2: a data access policy's per-operation Permission list is still not evaluated, so withdrawing aoss:WriteDocument never takes effect ===
Failing check: opensearchserverless.data-access-policy-read-permission-without-write-blocks-indexing
AWS: recorded behavior only (the data plane is unmodeled); the permission-to-operation map is documented in 'Supported operations and plugins in Amazon OpenSearch Serverless' (serverless-genref.html): aoss:WriteDocument -> PUT /<index>/_doc/<id>; aoss:ReadDocument -> GET /<index>/_doc/<id> and GET/POST /<index>/_search; aoss:DescribeIndex -> GET /<index>/_mapping. UpdateAccessPolicy REPLACES the policy document, so dropping a Permission entry really withdraws that permission. Recorded: once the `index/<collection>/*` rule is narrowed to `["aoss:WriteDocument" removed]`, the same aoss-signed PUT is refused with HTTP 403 and an OpenSearch error document whose `error.type` is the fixed text `forbidden`, the document's `_version` does not advance, and the reads whose permission was NOT withdrawn keep answering 200. The 403 and the fixed `forbidden` type are the requirement; index name, document id, collection name and version number are request/state values.
Emulator: no refusal is ever produced. The check performs its positive control, replaces the policy with the read-only document via UpdateAccessPolicy, then re-issues the identical write on a bounded poll waiting for the service to start refusing; the refusal never comes and the check is killed by its budget — the suite reports `Error: Check timed out after 60.0s`. Allowed and withdrawn are indistinguishable at the endpoint.
Why: a client that uses a data access policy as its authorization boundary gets none. A principal narrowed to read-only can still index, update and overwrite documents, and an operator who revokes write access sees UpdateAccessPolicy succeed while writes keep landing — a silent privilege escalation with no observable signal anywhere in the API.
Fix: the collection data-plane request handler's authorization step in the opensearchserverless service crate — resolve the signing principal, select the data access policies whose Principal matches and whose Rules' Resource matches `collection/<name>` and `index/<name>/<index>`, map the HTTP verb+path to the aoss permission it requires, and refuse with 403 plus an OpenSearch error body when that permission is absent, instead of serving every signed request unconditionally.

Traces (examples): Cause 1 — opensearchserverless.collection-endpoint-requires-aoss-sigv4-service-name. Cause 2 — opensearchserverless.data-access-policy-read-permission-without-write-blocks-indexing.

## Evidence
Cause 1 — opensearchserverless.collection-endpoint-requires-aoss-sigv4-service-name [mismatch]:
  Expected (AWS): the es-scoped PUT /<index>/_doc/<id> (identical to an accepted aoss-signed write in every other respect) -> HTTP 403 with body {"status": 403, "error": {"type": "forbidden", "reason": "403 Forbidden"}}.
  Actual (emulator): HTTP 201 with a successful write receipt. Suite detail: `AssertionError: expected HTTP 403 for a request signed with scope 'es' against an aoss collection endpoint, got HTTP 201: {'_index': 'sim2real-scope', '_id': 'scoped-es', '_version': 1, 'result': 'created', '_shards': {'total': 1, 'successful': 1, 'failed': 0}, '_seq_no': 1, '_primary_term': 1} -- an aoss endpoint that accepts the managed OpenSearch signing scope is not validating the credential scope at all`. ('sim2real-scope' / 'scoped-es' / _seq_no 1 are this run's request values; the requirement is the 403 and its fixed error document.)

Cause 2 — opensearchserverless.data-access-policy-read-permission-without-write-blocks-indexing [mismatch]:
  Expected (AWS): after UpdateAccessPolicy replaces the index rule with a Permission list that no longer contains aoss:WriteDocument, the identical PUT /<index>/_doc/<id> -> HTTP 403 with error.type == "forbidden", the document's _version frozen, while GET /<index>/_doc/<id> and POST /<index>/_search (aoss:ReadDocument retained) keep answering 200.
  Actual (emulator): no 403 is ever observed; the bounded re-probe loop runs until the check budget is exhausted. Suite detail: `Error: Check timed out after 60.0s`.

Context: both checks' positive controls are served correctly — an aoss-signed PUT/GET/_search against the published collectionEndpoint returns real OpenSearch document bodies — which is why these authorization-layer divergences are the ones now visible.

## Steps to Reproduce
Common setup:
1. opensearchserverless.create_security_policy(type='encryption', name=<e>, policy matching collection/<name>, aWSOwnedKey=true).
2. Create an IAM role holding aoss:APIAccessAll for the collection, assume it, and use those frozen credentials for every data-plane call. SigV4-sign with signing service name 'aoss' and send x-amz-content-sha256 (the payload's sha256 hex digest) on every body-bearing request.
3. opensearchserverless.create_access_policy(type='data', name=<d>, policy=[{Rules:[{ResourceType:'collection', Resource:['collection/<name>'], Permission:['aoss:CreateCollectionItems','aoss:DescribeCollectionItems']},{ResourceType:'index', Resource:['index/<name>/*'], Permission:['aoss:CreateIndex','aoss:DescribeIndex','aoss:UpdateIndex','aoss:ReadDocument','aoss:WriteDocument']}], Principal:['<the role ARN>']}]).
4. create_collection(name=<name>, type='SEARCH', encryptionConfig={'aWSOwnedKey': True}); poll batch_get_collection(ids=[<id>]) until status=='ACTIVE'; read collectionDetails[0].collectionEndpoint as <base>.

Cause 1 (SigV4 credential scope not validated):
5. PUT <base>/<index>/_doc/allowed with a JSON body, SigV4-signed with service name 'aoss' -> accepted 200/201 (positive control).
6. Re-send the identical request — same method, URL, body, credentials and clock — but sign it with service name 'es' instead of 'aoss'.
   -> EXPECTED: HTTP 403 with {"status": 403, "error": {"type": "forbidden", "reason": "403 Forbidden"}}.
   -> ACTUAL: HTTP 201 and the document is created.

Cause 2 (data access policy Permission list not evaluated):
5. PUT <base>/<index>/_doc/seed -> accepted; GET <base>/<index>/_doc/seed -> 200 with the stored _source (positive control).
6. get_access_policy(type='data', name=<d>) to read policyVersion, then update_access_policy(type='data', name=<d>, policyVersion=<v>, policy=<the same document with the index rule's Permission narrowed to ['aoss:ReadDocument']>).
7. Re-issue the identical PUT <base>/<index>/_doc/blocked on a bounded poll.
   -> EXPECTED: HTTP 403 with an OpenSearch error body whose error.type == 'forbidden'; _version does not advance; GET /_doc and POST /_search still 200.
   -> ACTUAL: the write keeps being accepted for the whole window and the check is killed by its budget; no refusal is ever returned.
