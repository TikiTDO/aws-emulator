Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__iam__lambda__logs__opensearch__s3.cfn-search-indexing-pipeline-alarm-config-workflow
Issue-Key: cloudwatch__iam__lambda__logs__opensearch__s3.cfn-search-indexing-pipeline-alarm-config-workflow|_assert_object_is_indexed#21|AssertionError|assert doc.get("_id") == key, (
Issue-Commit: cf4b8496ba0c958ea150f5d2d9329060f4b043af

# opensearch data plane does not percent-decode the document-id path segment, so a document indexed as <index>/_doc/docs%2Ffile.json reports _id 'docs%2Ffile.json'

> _Note: this issue was identified against the code base at commit `cf4b8496ba0c958ea150f5d2d9329060f4b043af`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDomain

## Description
Cause 1 of 1: the OpenSearch domain data plane stores and reports the raw, still-escaped path segment as the document _id.
AWS: recorded behavior only — the domain data plane is the OpenSearch engine REST API, not covered by the opensearch Smithy model. The engine percent-decodes each path segment before routing, so PUT /<index>/_doc/docs%2F<name>.json creates a document whose _id is the decoded literal `docs/<name>.json`; a subsequent GET /<index>/_doc/docs%2F<name>.json returns that document and every _search hit reports "_id": "docs/<name>.json". The response status is 201/200; nothing is refused.
Emulator: the escape survives into the stored id. The indexing pipeline's signed GET and _search both report the id with the escape intact: expected _id 'docs/<name>.json', got 'docs%2F<name>.json'.
Why: using a path-bearing key as the document id is the canonical S3-object -> OpenSearch-document pattern, and this is exactly what the check's Lambda indexer does. A client that writes with an escaped id and then reads with the decoded one (or vice versa) gets 404, two ids that denote one document are kept apart, and any consumer that reconstructs the source object key from _id reconstructs a key that does not exist in the bucket.
Fix: the domain data-plane REST router in the opensearch service crate — percent-decode each path segment (document id, index name, alias) once, before the id is resolved or stored, rather than passing the raw segment through to state.
Traces: cloudwatch__iam__lambda__logs__opensearch__s3.cfn-search-indexing-pipeline-alarm-config-workflow.

## Evidence
Replay output: AssertionError: the domain must decode the %2F path segment back to a literal slash: expected _id 'docs/sim2real-xsvc-doc-af62f733e00e.json', got 'docs%2Fsim2real-xsvc-doc-af62f733e00e.json'. Fixed text of the requirement: the _id reported by GET /<index>/_doc/<id> and by _search equals the percent-DECODED path segment; the variable parts (index name, object key, generated suffix) come from the request — 'docs/sim2real-xsvc-doc-af62f733e00e.json' is one run's value, not part of the requirement.

## Steps to Reproduce
1. CreateDomain and wait for a published Endpoint; create an index.
2. Send a SigV4-signed ('es') PUT to https://<endpoint>/<index>/_doc/docs%2Fexample.json with a JSON body.
3. GET https://<endpoint>/<index>/_doc/docs%2Fexample.json and POST https://<endpoint>/<index>/_search — both must report "_id": "docs/example.json"; the emulator reports "docs%2Fexample.json".
