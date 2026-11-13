Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearchserverless.index-lifecycle-transitions
Issue-Shape-Id: com.amazonaws.opensearchserverless#OpenSearchServerless
Issue-Key: opensearchserverless.index-lifecycle-transitions|run#27|AssertionError|assert message == expected_message, f"expected GetIndex message {expected_message!r}, got {message!r}"
Issue-Commit: aa1ad4990c47de4e76a00e85eb04af6a36763226

# GetIndex/UpdateIndex report a missing index with the wrong ResourceNotFoundException message ("The index '<n>' does not exist in the collection with Id '<id>'." instead of AWS's "Index not found: <indexName>")

> _Note: this issue was identified against the code base at commit `aa1ad4990c47de4e76a00e85eb04af6a36763226`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetIndex

## Description
ONE net-new root cause in the opensearchserverless refusal plane, grounded in the failing behavioral check opensearchserverless.index-lifecycle-transitions.

=== Cause 1 of 1: the index-not-found refusal message is not AWS's text ===
AWS: Smithy (services/service-com-amazonaws-opensearchserverless-opensearchserverless/smithy-model/com-amazonaws-opensearchserverless-opensearchserverless.json): GetIndex takes id (CollectionId, len 3..40, pattern ^[a-z0-9]{3,40}$) + indexName (IndexName), and models ResourceNotFoundException (HTTP 404) — "Thrown when accessing or deleting a resource that does not exist"; UpdateIndex takes the same id + indexName (+ indexSchema, a document) and models the same ResourceNotFoundException (404). The model does not fix the message text, so this is recorded behavior only: for an index name that addresses no index in an existing, ACTIVE collection — e.g. immediately after DeleteIndex — both GetIndex and UpdateIndex are refused 404 ResourceNotFoundException with the message `Index not found: <indexName>`. The message template is the fixed text `Index not found: ` followed by exactly ONE variable part, the `indexName` taken verbatim from the request; the collection id does NOT appear in it. (In the recording the variable part was the per-run name `sim2real-index-f8e78009c2c6` — that value fills the template, it is not part of it.)
Emulator: the status (404) and the error code (ResourceNotFoundException) are right, but the message is a different template with TWO variable parts: `The index '<indexName>' does not exist in the collection with Id '<collectionId>'.` — observed as "The index 'sim2real-index-f8e78009c2c6' does not exist in the collection with Id 'rdg0un2g53eo5x1rs3xqrx3t'." Both the fixed text and the variable set diverge: AWS's fixed prefix `Index not found: ` is absent, the name is quoted, and the collection id is added. The same wording is served by GetIndex, UpdateIndex and DeleteIndex because all three read it from one shared helper, so every index-addressing refusal in this crate carries it.
Why: the HTTP status, the error code and the error message are all part of the contract. A client that distinguishes a deleted index from a deleted collection by matching the refusal message — AWS words the collection case separately — never recognises the index case, and anything that logs, asserts on, or surfaces AWS's message to a user (index-existence probes, CDK/Terraform-style provisioners that tolerate a known not-found text, test suites pinned to AWS) sees text it cannot match. It also leaks the collection id into a message AWS keeps to the index name.
Fix: the shared index-not-found message helper in the opensearchserverless crate's state module — `index_not_found_message(index_name, collection_id)` at services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/state.rs:2471-2473 — must render AWS's template (fixed text `Index not found: ` + the request's indexName, and nothing else). Its three callers then need no change: GetIndex (services/.../src/ops/get_index.rs:183-193), UpdateIndex (services/.../src/ops/update_index.rs:198-208) and DeleteIndex, which all construct ResourceNotFoundException from this helper. The sibling helper `index_collection_not_found_message` (state.rs:2461-2463), used when the ID names no collection, is a DIFFERENT refusal and is not in scope here; if the collection-id-bearing wording is dropped from the index message, the collection case must keep wording its own refusal.
Note on neighbouring issues: open issue 20261029T191222Z-opensearchserverless-5-net-new-causes-in-the-state.md carries a different message-text defect (BatchGetVpcEndpoint's NOT_FOUND errorMessage) and a different UpdateIndex defect (schema replacement rather than merge); open issue 20261029T175047Z-opensearchserverless-updateindex-validation.md concerns UpdateIndex writing to a non-ACTIVE collection. Neither covers this message template, and this check was not marked owned by either.
Traces: opensearchserverless.index-lifecycle-transitions.

## Evidence
Failing check opensearchserverless.index-lifecycle-transitions (mismatch):

  AssertionError: expected GetIndex message 'Index not found: sim2real-index-f8e78009c2c6', got "The index 'sim2real-index-f8e78009c2c6' does not exist in the collection with Id 'rdg0un2g53eo5x1rs3xqrx3t'."

The check first asserts the code, which PASSES: GetIndex after DeleteIndex is refused 404 with Error.Code == 'ResourceNotFoundException', and UpdateIndex on the same deleted index is likewise refused with 'ResourceNotFoundException'. Only the message diverges, for both operations (the check asserts the identical expected text for GetIndex and UpdateIndex).

Expected (AWS): Error.Message == 'Index not found: <indexName>' — fixed text 'Index not found: ' plus the request's indexName.
Actual (emulator): Error.Message == "The index '<indexName>' does not exist in the collection with Id '<collectionId>'." — a different fixed text, the name quoted, and the collection id appended from state.

Source of the actual text in the checkout:
  services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/state.rs:2471
    pub fn index_not_found_message(index_name: &str, collection_id: &str) -> String {
        format!("The index '{index_name}' does not exist in the collection with Id '{collection_id}'.")
  called by get_index.rs:186 and update_index.rs:201 (and by DeleteIndex, per that helper's own doc comment: "GetIndex, UpdateIndex and DeleteIndex each address an existing index this way").

The index name and collection id above are per-run values and are not themselves requirements; the divergence is in the template.

## Steps to Reproduce
1. aoss = boto3 client('opensearchserverless').
2. create_collection(name=<coll>, type='SEARCH', encryptionConfig={'aWSOwnedKey': True}); note createCollectionDetail.id.
3. Grant the caller data access: create_access_policy(type='data', name=<p>, policy=[{Rules:[{ResourceType:'collection',Resource:['collection/<coll>'],Permission:['aoss:CreateCollectionItems','aoss:DescribeCollectionItems','aoss:UpdateCollectionItems']},{ResourceType:'index',Resource:['index/<coll>/*'],Permission:['aoss:CreateIndex','aoss:DescribeIndex','aoss:UpdateIndex','aoss:DeleteIndex','aoss:ReadDocument','aoss:WriteDocument']}],Principal:[<caller IAM role arn>]}]) and create_security_policy(type='network', name=<n>, policy=[{Rules:[{ResourceType:'collection',Resource:['collection/<coll>']}],AllowFromPublic:True}]).
4. Poll batch_get_collection(ids=[<id>]) until collectionDetails[0].status == 'ACTIVE'.
5. create_index(id=<id>, indexName=<idx>, indexSchema={'mappings': {'properties': {'title': {'type': 'text'}}}}).
6. delete_index(id=<id>, indexName=<idx>).
7. get_index(id=<id>, indexName=<idx>) -> 404 ResourceNotFoundException. Inspect Error.Message: AWS returns 'Index not found: <idx>'; the emulator returns "The index '<idx>' does not exist in the collection with Id '<id>'."
8. update_index(id=<id>, indexName=<idx>, indexSchema={'mappings': {'properties': {'title': {'type': 'text'}}}}) -> same ResourceNotFoundException with the same diverging message.
(DeleteIndex on the already-deleted index exercises the same shared helper.)
