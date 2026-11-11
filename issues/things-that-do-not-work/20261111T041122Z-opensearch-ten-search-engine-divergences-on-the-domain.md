Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.bm25-relevance-ordering-and-explain-score-arithmetic opensearch.search-aggregation-query-bucket-ordering opensearch.scroll-pagination-returns-each-document-exactly-once opensearch.update-by-query-script-touches-exactly-the-matched-documents opensearch.knn-vector-query-nearest-neighbour-ordering opensearch.point-in-time-search-after-walk-isolated-from-concurrent-indexing opensearch.nested-mapping-isolates-sub-documents-from-flattened-object-matching opensearch.custom-analyzer-token-stream-drives-search-match opensearch.osq-deep-opensearch-2-template-priority-composition-and-equal-priority-overlap-rejected opensearch.cluster-settings-flat-form-only-and-auto-create-index-false-refuses-implicit-index
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.bm25-relevance-ordering-and-explain-score-arithmetic|run#17|AssertionError|assert ranked and ranked[0][0] == "zebra", (
Issue-Key: opensearch.search-aggregation-query-bucket-ordering|run#22|AssertionError|assert [b["key"] for b in buckets] == ["red", "blue", "green"], (
Issue-Key: opensearch.scroll-pagination-returns-each-document-exactly-once|run#14|AssertionError|assert isinstance(scroll_id, str) and scroll_id, (
Issue-Key: opensearch.update-by-query-script-touches-exactly-the-matched-documents|run#11|KeyError|pre_versions = {h["_id"]: h["_version"] for h in baseline["hits"]["hits"]}
Issue-Key: opensearch.knn-vector-query-nearest-neighbour-ordering|run#17|AssertionError|assert status == 200, f"k-NN _search failed with HTTP {status}: {result}"
Issue-Key: opensearch.point-in-time-search-after-walk-isolated-from-concurrent-indexing|_prefix_total#10|AssertionError|assert status == 200, f"a {prefix!r} count returned HTTP {status!r}: {result!r}"
Issue-Key: opensearch.nested-mapping-isolates-sub-documents-from-flattened-object-matching|run#18|AssertionError|assert _total(unrefreshed) == 0, (
Issue-Key: opensearch.custom-analyzer-token-stream-drives-search-match|run#8|AssertionError|assert status == 200, f"_analyze returned HTTP {status!r}: {analyzed!r}"
Issue-Key: opensearch.osq-deep-opensearch-2-template-priority-composition-and-equal-priority-overlap-rejected|run#28|AssertionError|assert lost.get("from_component", {}).get("type") == "text", (
Issue-Key: opensearch.cluster-settings-flat-form-only-and-auto-create-index-false-refuses-implicit-index|run#18|AssertionError|assert refused_reason == f"no such index [{refused_index}]", (
Issue-Commit: 181b4db8a49f9f99cf56c1b2f4aacf19bf82375e

# opensearch: ten search-engine divergences on the domain data plane — no BM25 scoring, no terms buckets, no _scroll_id, no _version on hits, knn/prefix queries unregistered, un-refreshed writes searchable, no asciifolding filter, dynamic mapping not reflected, and an over-worded auto_create_index refusal (10 causes)

> _Note: this issue was identified against the code base at commit `181b4db8a49f9f99cf56c1b2f4aacf19bf82375e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
OpenSearch domain REST data plane search/index APIs (es:ESHttpGet / es:ESHttpPost / es:ESHttpPut)

## Description
Ten independent defects in the emulator's OpenSearch search/index engine on a created domain's published endpoint. Unlike the routing gaps filed separately, each of these endpoints IS routed and answers 200 (or a wrongly-worded 4xx) — the computed result is wrong. Each cause must be closed separately. Ten causes.

Note: open issue 20261109T062930Z-opensearch-the-domain-search-api-silently-ignores-the.md covers the search API ignoring `highlight` and `sort`, and 20261111T000041Z-opensearch-3-net-new-causes-the-data-plane.md covers flat_settings / settings JSON types / read-only blocks. Neither covers any cause below, and the replay marks none of these checks as owned.

--- Cause 1 of 10: relevance scoring is constant — every hit scores 1.0, so BM25 ordering never happens ---
AWS: recorded behavior only (the scoring rule is OpenSearch's BM25 default similarity). A full-text match query returns hits ordered by descending _score, where a term held by few documents (low df, high idf) outweighs a term held by many; the document holding the df=1 term ranks first, and `explain` returns the idf/tf arithmetic that produced the score.
Emulator: every hit comes back with `_score: 1.0` and `max_score: 1.0`, so the result order is arbitrary (insertion order) — the replay saw `[('c000', 1.0), ('c001', 1.0), ('c002', 1.0), ('c003', 1.0), ('c004', 1.0)]` for a query whose rare-term document had to come first.
Why: relevance ranking is the point of search. A client that takes the top N hits gets arbitrary documents, and nothing in the response reveals that ranking was not performed.
Fix: the search handler's scoring component — implement BM25 term scoring (tf, df/idf, field-length norm) and sort hits by _score descending, and make `explain` report the same arithmetic.
Traces: opensearch.bm25-relevance-ordering-and-explain-score-arithmetic.

--- Cause 2 of 10: terms aggregation returns no buckets ---
AWS: recorded behavior only. A `terms` aggregation over a keyword field answers HTTP 200 with one bucket per distinct value, ordered by `doc_count` descending, each bucket carrying `key` and `doc_count`.
Emulator: HTTP 200 but the aggregation's `buckets` array is EMPTY (`got []`) even though the matching documents exist and are searchable.
Why: every aggregation-driven client (dashboards, facets, counts-by-value) reads an empty result as 'no data' rather than 'unsupported', and silently reports zero everywhere.
Fix: the search handler's aggregation component — evaluate `aggs` against the matched document set and emit doc_count-descending buckets.
Traces: opensearch.search-aggregation-query-bucket-ordering.

--- Cause 3 of 10: _search?scroll=<keep_alive> returns no _scroll_id ---
AWS: recorded behavior only. A search issued with `scroll=<duration>` answers HTTP 200 with a top-level `_scroll_id` string; POST /_search/scroll with that id walks the remaining pages and returns each document exactly once until a page comes back empty.
Emulator: HTTP 200 with the first page of hits but NO `_scroll_id` member at all, so there is nothing to page with.
Why: a client cannot iterate a result set larger than one page; code that reads `_scroll_id` gets a KeyError/None and either crashes or silently processes only the first page, losing the rest of the data.
Fix: the search handler — mint and return a `_scroll_id` when `scroll` is present, and keep the scroll cursor state that /_search/scroll advances and expires.
Traces: opensearch.scroll-pagination-returns-each-document-exactly-once.

--- Cause 4 of 10: search hits omit `_version` ---
AWS: recorded behavior only. A search hit carries `_version` (the document's monotonically increasing version) alongside `_index`, `_id`, `_score` and `_source`, which is how a client proves an update actually touched a document.
Emulator: hits carry no `_version` member — reading it raises KeyError: '_version'.
Why: optimistic-concurrency and change-detection clients cannot tell whether a document was modified; the member is absent rather than stale, so the omission surfaces as a crash in the client, not as a recoverable value.
Fix: the search handler's hit serializer plus the per-document version counter in the index state, so that writes increment it and hits report it.
Traces: opensearch.update-by-query-script-touches-exactly-the-matched-documents.

--- Cause 5 of 10: the `knn` query type is not registered ---
AWS: recorded behavior only. On a knn_vector-mapped index, `{"query":{"knn":{"<field>":{"vector":[...],"k":N}}}}` answers HTTP 200 with the N nearest neighbours ordered by decreasing similarity.
Emulator: HTTP 400 `{"error": {"root_cause": [{"type": "parsing_exception", "reason": "no [query] registered for [knn]"}], "type": "parsing_exception", "reason": "no [query] registered for [knn]"}, "status": 400}` — the fixed text is `no [query] registered for [` + the query name + `]`.
Why: vector search is unusable. A client is told its query is syntactically unknown, which it cannot distinguish from a typo in its own request.
Fix: the search handler's query-DSL parser and the vector-similarity evaluator for knn_vector fields.
Traces: opensearch.knn-vector-query-nearest-neighbour-ordering.

--- Cause 6 of 10: the `prefix` query type is not registered ---
AWS: recorded behavior only. `{"query":{"prefix":{"<field>":"<value>"}}}` answers HTTP 200 and matches exactly the documents whose field term starts with the given prefix; the same body under _count returns their count.
Emulator: HTTP 400 `parsing_exception` with reason `no [query] registered for [prefix]`.
Why: a core, long-standing query type is rejected as unknown, so prefix/autocomplete-style clients fail outright, and any count built on a prefix query cannot be taken.
Fix: the search handler's query-DSL parser — register `prefix` and evaluate it against the field's indexed terms.
Traces: opensearch.point-in-time-search-after-walk-isolated-from-concurrent-indexing.

--- Cause 7 of 10: writes are searchable before _refresh ---
AWS: recorded behavior only. With the index's default refresh interval, a document indexed without `refresh=true`/`refresh=wait_for` is NOT yet visible to _search; a search before the refresh returns `hits.total.value: 0`, and only after POST /<index>/_refresh (or the interval elapsing) does it appear. A get-by-id, by contrast, is realtime.
Emulator: the search immediately before _refresh already returned the freshly indexed documents (`2 hits`, `hits.total.value: 2`), i.e. un-refreshed writes are published to search.
Why: the emulator is strictly more visible than AWS, so a client's refresh handling is never exercised and read-your-writes bugs that would fail against AWS pass here. It also makes the near-real-time contract untestable in either direction.
Fix: the index/document write path and the search handler — hold un-refreshed writes out of the searchable segment until _refresh, the refresh interval, or an explicit refresh parameter publishes them.
Traces: opensearch.nested-mapping-isolates-sub-documents-from-flattened-object-matching.

--- Cause 8 of 10: the `asciifolding` built-in token filter is missing from _analyze / custom analyzers ---
AWS: recorded behavior only. `asciifolding` is a built-in token filter; an index-settings custom analyzer naming it is accepted, and POST /<index>/_analyze with that analyzer answers HTTP 200 with the folded token stream (e.g. accented characters folded to ASCII), which then drives match behaviour at search time.
Emulator: HTTP 400 `{"error": {"root_cause": [{"type": "illegal_argument_exception", "reason": "failed to find token filter under [asciifolding]"}], "type": "illegal_argument_exception", "reason": "failed to find token filter under [asciifolding]"}, "status": 400}` — the fixed text is `failed to find token filter under [` + the filter name + `]`.
Why: a built-in filter is reported as nonexistent, so a client's custom analyzer is rejected and no accent-insensitive matching is possible; the message blames the user's settings for an engine gap.
Fix: the analysis component behind _analyze and index settings — register the built-in token filters (asciifolding among them) so custom analyzers can reference them.
Traces: opensearch.custom-analyzer-token-stream-drives-search-match.

--- Cause 9 of 10: dynamically mapped fields are not reflected in the index mapping ---
AWS: recorded behavior only. When a document introduces a field that no index or component template covers, dynamic mapping assigns it a type (a JSON string becomes `text` with a `keyword` subfield) and GET /<index>/_mapping reports that type thereafter.
Emulator: for an index with no component template, the dynamically introduced field's mapping entry is absent — reading its `type` yields None instead of `text`.
Why: a client that inspects the mapping to decide how to query a field (text vs keyword, analyzed vs exact) sees nothing and cannot tell a missing field from an unmapped one; template-composition precedence (template-supplied type vs dynamically derived type) becomes unobservable.
Fix: the document write path's dynamic-mapping component plus the mapping serializer, so a newly seen field is recorded with its derived type and returned by GET _mapping.
Traces: opensearch.osq-deep-opensearch-2-template-priority-composition-and-equal-priority-overlap-rejected.

--- Cause 10 of 10: the auto_create_index=false refusal message is over-worded ---
AWS: recorded behavior only. With cluster setting `action.auto_create_index` set to `false`, indexing into an index that does not exist is refused with HTTP 404 and an `index_not_found_exception` whose reason is exactly `no such index [` + the requested index name + `]` — the fixed text is `no such index [` and `]`, and the only variable part is the index name from the request.
Emulator: the same 404/index_not_found_exception but with the reason `no such index [<index>] and [action.auto_create_index] is [false]`, i.e. an extra ` and [action.auto_create_index] is [false]` suffix appended to both `root_cause[0].reason` and the top-level `reason`.
Why: clients and log pipelines that match the refusal reason exactly (or parse the index name out of it) do not recognise the emulator's wording, so a refusal AWS words one way is classified differently or not at all.
Fix: the index-not-found refusal's message in the document write path's validation — emit AWS's fixed text with the request's index name and nothing more.
Traces: opensearch.cluster-settings-flat-form-only-and-auto-create-index-false-refuses-implicit-index.

## Evidence
Verbatim from the replay (per-run index/document names are values, not requirements):

1. BM25: "'zebra common' ranked [('c000', 1.0), ('c001', 1.0), ('c002', 1.0), ('c003', 1.0), ('c004', 1.0)]; the document holding the df=1 term must come first" — expected descending _score with the rare-term document first; actual all _score 1.0.
2. terms agg: "terms buckets must be ordered by doc_count desc, got []" — expected ["red","blue","green"] buckets; actual empty.
3. scroll: "the first scroll page carried no _scroll_id, so there is nothing to page with: {'timed_out': False, '_shards': {...}, 'hits': {'total': {'value': 25, ...}, 'max_score': 1.0, 'hits': [...10 hits...]}}" — expected a top-level _scroll_id string; actual member absent.
4. _version: "KeyError: '_version'" raised reading h["_version"] from search hits — expected _version on each hit; actual absent.
5. knn: "k-NN _search failed with HTTP 400: {'error': {'root_cause': [{'type': 'parsing_exception', 'reason': 'no [query] registered for [knn]'}], 'type': 'parsing_exception', 'reason': 'no [query] registered for [knn]'}, 'status': 400}" — expected HTTP 200 with k nearest neighbours.
6. prefix: "a 'orig-' count returned HTTP 400: {'error': {'root_cause': [{'type': 'parsing_exception', 'reason': 'no [query] registered for [prefix]'}], ...}, 'status': 400}" — expected HTTP 200 with the prefix-matched count.
7. refresh: "documents were searchable before _refresh (2 hits) -- the engine published un-refreshed writes to search: {... 'hits': {'total': {'value': 2, 'relation': 'eq'}, ...}}" — expected hits.total.value 0 before _refresh.
8. asciifolding: "_analyze returned HTTP 400: {'error': {'root_cause': [{'type': 'illegal_argument_exception', 'reason': 'failed to find token filter under [asciifolding]'}], ...}, 'status': 400}" — expected HTTP 200 with the folded token stream.
9. dynamic mapping: "'<index>' has no component template, so 'from_component' must be dynamically mapped text, got None" — expected mapping type "text"; actual no mapping entry.
10. auto_create_index: "the refusal reason is 'no such index [<index>] and [action.auto_create_index] is [false]', expected the exact 'no such index [<index>]' ... {'error': {'root_cause': [{'type': 'index_not_found_exception', 'reason': 'no such index [<index>] and [action.auto_create_index] is [false]'}], 'type': 'index_not_found_exception', 'reason': 'no such index [<index>] and [action.auto_create_index] is [false]'}, 'status': 404}".

## Steps to Reproduce
1. CreateDomain (EngineVersion OpenSearch_2.11, single t3.small.search node, no dedicated master, no zone awareness, gp3 10 GiB) and wait for Processing=false; read DomainStatus.Endpoint.
2. Scoring: create an index, index ~30 documents sharing one common term and exactly one holding a rare term, _refresh, then POST /<index>/_search with a match query over both terms — observe every hit at _score 1.0 and the rare-term document not first.
3. Aggregation: POST /<index>/_search with a `terms` aggregation over a keyword field — observe an empty `buckets` array.
4. Scroll: POST /<index>/_search?scroll=1m&size=10 over 25 documents — observe no `_scroll_id` in the response.
5. Version: POST /<index>/_search and read `_version` on a hit — observe the member is absent.
6. knn: map a knn_vector field and POST /<index>/_search with {"query":{"knn":{...}}} — observe 400 parsing_exception 'no [query] registered for [knn]'.
7. prefix: POST /<index>/_count with {"query":{"prefix":{"<field>":"orig-"}}} — observe 400 parsing_exception 'no [query] registered for [prefix]'.
8. Refresh: index documents WITHOUT refresh, immediately POST /<index>/_search — observe the documents already returned (expected hits.total.value 0 until _refresh).
9. asciifolding: create an index whose settings define a custom analyzer with the `asciifolding` filter, then POST /<index>/_analyze with that analyzer — observe 400 illegal_argument_exception 'failed to find token filter under [asciifolding]'.
10. Dynamic mapping: create an index covered by no component template, index a document with a new string field, then GET /<index>/_mapping — observe the field has no mapping entry (expected type "text").
11. auto_create_index: PUT /_cluster/settings with action.auto_create_index=false, then index into a nonexistent index — observe the 404 reason carries the extra ' and [action.auto_create_index] is [false]' suffix.
