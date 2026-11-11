Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__iam__lambda__logs__opensearch__s3.cfn-search-indexing-pipeline-workflow
Issue-Key: cloudwatch__iam__lambda__logs__opensearch__s3.cfn-search-indexing-pipeline-workflow|_assert_object_is_indexed#21|AssertionError|assert doc.get("_id") == key, (
Issue-Commit: 94350cf775acbc496950503935930c48964de18f

# cloudwatch__iam__lambda__logs__opensearch__s3: the OpenSearch data plane does not percent-decode the document id in the index path, so a key containing %2F is indexed under the escaped literal

> _Note: this issue was identified against the code base at commit `94350cf775acbc496950503935930c48964de18f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDomain

## Description
Cause 1 of 1: the `_id` path segment of an OpenSearch index request is taken as raw bytes instead of being percent-decoded, so the stored document id keeps the escape sequences the client had to use.
AWS: an OpenSearch domain's REST API takes the document id from the request target's path segment, and that segment is percent-decoded before it becomes the `_id`. A client indexing a document whose id contains a slash MUST escape it as `%2F` (an unescaped `/` would start a new path segment), and the domain stores and returns the DECODED id — `GET /<index>/_doc/<escaped-id>` and a search hit both report `_id` with a literal `/`. So for the id `docs/<name>.json` the client sends `.../_doc/docs%2F<name>.json` and `_id` reads back as `docs/<name>.json`. (Recorded behavior; the fixed rule is the decoding, the id itself comes from the S3 object key the pipeline indexed.)
Emulator: the escape is not decoded. For the same request the indexed document's `_id` is `docs%2F<name>.json` — the percent-escaped form — so it differs from the key the client indexed.
Why: a client that indexes documents keyed by anything hierarchical (an S3 object key is the canonical case, and is exactly what this ingestion pipeline does) can never address or correlate its own documents: the id it looks up by, deletes by, or joins against its source store does not exist in the index, and every id containing a reserved character is silently mangled. The divergence is invisible until a lookup by the real key returns nothing.
Fix: the OpenSearch data-plane request routing/handler for the `_doc`/`_create`/`_update` paths — percent-decode the id path segment (and the index name segment) before it is used as the document key.
Related open issue: 20261111T000041Z-cloudwatch-iam-lambda-logs-opensearch-s3-opensearch-data-plane-does-not-percent-decode-the.md reports the same decoding fault and owns the sibling check `cfn-search-indexing-pipeline-alarm-config-workflow` in this same section; it does not carry THIS check id, so this check is not covered by it.
Traces: cloudwatch__iam__lambda__logs__opensearch__s3.cfn-search-indexing-pipeline-workflow.

## Evidence
Behavioral suite replay (simparity-behavioral-aws-v1), `### cloudwatch__iam__lambda__logs__opensearch__s3` section (0/2 passed).

[mismatch] cloudwatch__iam__lambda__logs__opensearch__s3.cfn-search-indexing-pipeline-workflow
  AssertionError: the domain must decode the %2F path segment back to a literal slash: expected _id 'docs/sim2real-xsvc-doc-<id>.json', got 'docs%2Fsim2real-xsvc-doc-<id>.json'

The document id above is the per-run S3 object key the pipeline indexed; the requirement is that the id path segment is percent-decoded, not the literal key.

## Steps to Reproduce
1. git_clone the repo; build and boot the emulator from the checkout.
2. Create an OpenSearch domain and index a document whose id contains a slash, sending it percent-escaped in the path: PUT /<index>/_doc/docs%2F<name>.json (SigV4-signed to the domain endpoint).
3. Read the document back (GET /<index>/_doc/docs%2F<name>.json, or search the index) and inspect `_id`.
4. AWS reports `_id` = 'docs/<name>.json' (decoded); the emulator reports 'docs%2F<name>.json'.
5. Replay cloudwatch__iam__lambda__logs__opensearch__s3.cfn-search-indexing-pipeline-workflow.
