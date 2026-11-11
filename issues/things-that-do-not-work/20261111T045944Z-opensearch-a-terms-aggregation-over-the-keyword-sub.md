Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.sorting-and-aggregating-a-text-field-rejected-while-its-keyword-subfield-serves-both
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.sorting-and-aggregating-a-text-field-rejected-while-its-keyword-subfield-serves-both|run#20|AssertionError|assert keyword_buckets == {t: 1 for t in SORTED_TITLES}, (
Issue-Commit: 2d1f8bf5471c7181f318f860d2c7fc1188aa0d11

# opensearch: a terms aggregation over the keyword sub-field of a text multi-field answers HTTP 200 with an empty buckets array, so facet/count-by-value reads return nothing (1 cause)

> _Note: this issue was identified against the code base at commit `2d1f8bf5471c7181f318f860d2c7fc1188aa0d11`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
OpenSearch domain REST data plane POST /<index>/_search with aggs (es:ESHttpPost)

## Description
One cause.

Cause 1 of 1: a `terms` aggregation targeting the `keyword` sub-field of a `text` multi-field (mapping `{"title": {"type": "text", "fields": {"raw": {"type": "keyword"}}}}`, aggregated on `title.raw`) is accepted with HTTP 200 but the aggregation's `buckets` array comes back EMPTY, even though the documents exist, are refreshed, and both other read paths over the same mapping work in the same run (a `match` on the analysed `title` finds all three documents and a `sort` on `title.raw` orders them lexicographically). Because the aggregation control never produces buckets, the check cannot even reach the refusal it exists to prove — that sorting/aggregating the analysed `text` field itself is refused HTTP 400 with search_phase_execution_exception wrapping illegal_argument_exception.

Note: open issue 20261111T041122Z-opensearch-ten-search-engine-divergences-on-the-domain.md describes an empty-buckets symptom as its Cause 2, for a terms aggregation over a plain keyword field; the replay marks this check as neither already-filed nor owned by it, and this trajectory adds the multi-field sub-field path (`<text field>.<keyword sub-field>`) — if one aggregation evaluator serves both, closing it must be verified against this check's id as well.

AWS: recorded behavior only (the domain's REST data plane is outside the opensearch Smithy model; the model covers CreateDomain/DescribeDomain and the endpoint it publishes). A `terms` aggregation over a keyword field — including the keyword sub-field of a text multi-field — answers HTTP 200 with one bucket per distinct value, ordered by `doc_count` descending, each bucket carrying `key` (the exact unanalysed term) and `doc_count`. With one document per distinct title on a single-shard index the counts are exact: one bucket per title, doc_count 1.
Emulator: HTTP 200, but `aggregations.titles.buckets` is an empty array — the replay's assertion reads `AssertionError: terms over the keyword sub-field must yield one bucket per title; got {}`. The preceding `match` over the analysed field and the `sort` over the same `title.raw` sub-field both returned the expected three documents in the same run, so the documents and the sub-field's terms are present; only the aggregation produces nothing.
Why: a client reads an empty `buckets` array as 'no data', not as 'unsupported'. Every facet, dashboard panel or count-by-value built on a keyword sub-field silently reports zero everywhere, with a 200 status and no error anywhere in the response to reveal that the aggregation was never evaluated. The usual, recommended mapping for 'full-text search plus exact aggregation on the same field' is exactly a text field with a keyword sub-field, so this is the common case rather than an edge one.
Fix: the search handler's aggregation component — resolve a dotted `field` to the sub-field of a multi-field mapping, evaluate `terms` against that sub-field's indexed terms over the matched document set (the whole index when `size: 0` with no query), and emit doc_count-descending buckets carrying `key` and `doc_count`.
Traces: opensearch.sorting-and-aggregating-a-text-field-rejected-while-its-keyword-subfield-serves-both.

## Evidence
Expected (recorded AWS): POST /<index>/_search with body {"size": 0, "aggs": {"titles": {"terms": {"field": "title.raw"}}}} over a single-shard index holding three documents whose `title` values are distinct answers HTTP 200 with three buckets, one per distinct title, each doc_count 1 (bucket keys are the documents' own values, not a requirement in themselves; the requirement is one bucket per distinct value of the aggregated sub-field with its exact count).

Actual (emulator), verbatim from the replay:
  #### [mismatch] opensearch.sorting-and-aggregating-a-text-field-rejected-while-its-keyword-subfield-serves-both
  File "check_opensearch_sorting_and_aggregating_a_text_field_rejected_while_its_keyword_subfield_serves_both.py", line 318, in run
  assert keyword_buckets == {t: 1 for t in SORTED_TITLES}, (
  AssertionError: terms over the keyword sub-field must yield one bucket per title; got {}

The status was 200 (the preceding status assertion passed); the divergence is the empty `aggregations.titles.buckets`. Controls in the same run that DID pass: a `match` query on the analysed `title` returned all three documents, and a `sort` on `title.raw` returned them in lexicographic order.

## Steps to Reproduce
1. CreateDomain (EngineVersion OpenSearch_2.11, single t3.small.search node, DedicatedMasterEnabled=false, ZoneAwarenessEnabled=false, EBS gp3 10 GiB); wait for Processing=false and read DomainStatus.Endpoint.
2. PUT /<index> with settings {index: {number_of_shards: 1, number_of_replicas: 0}} and mappings {properties: {title: {type: "text", fields: {raw: {type: "keyword"}}}}}.
3. PUT /<index>/_doc/d1, d2, d3 with three distinct multi-word `title` values whose lexicographic order differs from insertion order; POST /<index>/_refresh.
4. POST /<index>/_search {"query": {"match": {"title": "<shared word>"}}} — all three documents are found (control passes).
5. POST /<index>/_search {"sort": [{"title.raw": "asc"}]} — documents come back lexicographically ordered (control passes).
6. POST /<index>/_search {"size": 0, "aggs": {"titles": {"terms": {"field": "title.raw"}}}} — expect HTTP 200 with one bucket per distinct title (doc_count 1 each); observe HTTP 200 with `buckets` empty.
