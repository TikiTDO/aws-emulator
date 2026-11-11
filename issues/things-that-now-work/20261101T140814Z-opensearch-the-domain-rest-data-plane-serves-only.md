Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.bm25-relevance-ordering-and-explain-score-arithmetic opensearch.bulk-index-partial-failure-item-errors-rejected opensearch.closed-index-search-rejected-and-reopen-restores-documents opensearch.closed-point-in-time-reader-rejected opensearch.cluster-health-and-shard-allocation-match-domain-config opensearch.msearch-responses-are-positional-and-isolate-a-failing-slot opensearch.osc-immutable-number-of-shards-update-rejected opensearch.osq-deep-opensearch-2-async-delete-by-query-task-completes-with-exact-subset opensearch.point-in-time-search-after-walk-isolated-from-concurrent-indexing opensearch.reindex-into-keyword-mapping-preserves-documents-and-enables-term-match opensearch.restore-over-open-index-rejected opensearch.scroll-pagination-returns-each-document-exactly-once opensearch.search-aggregation-query-bucket-ordering opensearch.snapshot-restore-index-round-trip opensearch.update-by-query-script-touches-exactly-the-matched-documents opensearch.write-alias-rollover-routes-new-writes-to-new-backing-index opensearch.cluster-settings-put-is-refused-for-a-non-allowlisted-property opensearch.disabled-refresh-interval-hides-document-from-search-until-refresh opensearch.osq-deep-opensearch-2-custom-routing-selects-the-shard-for-get-and-search opensearch.index-mapping-conflict-rejected opensearch.knn-vector-query-nearest-neighbour-ordering opensearch.nested-mapping-isolates-sub-documents-from-flattened-object-matching opensearch.custom-analyzer-token-stream-drives-search-match opensearch.os-index-document-search-roundtrip opensearch.osx-deep-opensearch-3-blue-green-scale-out-data-continuity opensearch.osq-deep-opensearch-2-template-priority-composition-and-equal-priority-overlap-rejected opensearch.concurrent-conditional-writes-elect-exactly-one-winner opensearch.document-optimistic-concurrency-version-conflict-rejected opensearch.osq-deep-opensearch-2-highlight-wraps-matched-term-on-text-but-not-keyword-sibling opensearch.sorting-and-aggregating-a-text-field-rejected-while-its-keyword-subfield-serves-both opensearch.osc-deep-opensearch-1-over-replicated-index-yellow-then-green
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.bm25-relevance-ordering-and-explain-score-arithmetic|_bulk_load#13|AssertionError|assert status == 200, f"_bulk returned HTTP {status!r}: {body!r}"
Issue-Key: opensearch.bulk-index-partial-failure-item-errors-rejected|run#10|AssertionError|assert status == 200, (
Issue-Key: opensearch.closed-index-search-rejected-and-reopen-restores-documents|_bulk_load#8|AssertionError|assert status == 200, f"_bulk returned HTTP {status!r}: {body!r}"
Issue-Key: opensearch.closed-point-in-time-reader-rejected|_bulk_load#6|AssertionError|assert status == 200, f"_bulk returned HTTP {status!r}: {body!r}"
Issue-Key: opensearch.cluster-health-and-shard-allocation-match-domain-config|_bulk_load#11|AssertionError|assert status == 200, f"_bulk returned HTTP {status!r}: {body!r}"
Issue-Key: opensearch.msearch-responses-are-positional-and-isolate-a-failing-slot|_bulk_load#6|AssertionError|assert status == 200, f"_bulk returned HTTP {status!r}, expected 200: {body!r}"
Issue-Key: opensearch.osc-immutable-number-of-shards-update-rejected|_bulk_load#7|AssertionError|assert status == 200, f"_bulk returned HTTP {status!r}: {body!r}"
Issue-Key: opensearch.osq-deep-opensearch-2-async-delete-by-query-task-completes-with-exact-subset|run#11|AssertionError|assert status == 200 and body.get("errors") is False, f"_bulk returned HTTP {status!r}: {body!r}"
Issue-Key: opensearch.point-in-time-search-after-walk-isolated-from-concurrent-indexing|_bulk_load#7|AssertionError|assert status == 200, f"_bulk returned HTTP {status!r}: {body!r}"
Issue-Key: opensearch.reindex-into-keyword-mapping-preserves-documents-and-enables-term-match|_bulk_load#9|AssertionError|assert status == 200, f"_bulk returned HTTP {status!r}: {body!r}"
Issue-Key: opensearch.restore-over-open-index-rejected|_bulk_load#10|AssertionError|assert status == 200, f"_bulk returned HTTP {status!r}: {body!r}"
Issue-Key: opensearch.scroll-pagination-returns-each-document-exactly-once|_bulk_load#8|AssertionError|assert status == 200, f"_bulk returned HTTP {status!r}: {body!r}"
Issue-Key: opensearch.search-aggregation-query-bucket-ordering|run#10|AssertionError|assert status == 200, f"_bulk load failed with HTTP {status}: {bulk}"
Issue-Key: opensearch.snapshot-restore-index-round-trip|_bulk_load#9|AssertionError|assert status == 200, f"_bulk returned HTTP {status!r}: {body!r}"
Issue-Key: opensearch.update-by-query-script-touches-exactly-the-matched-documents|_bulk_load#8|AssertionError|assert status == 200, f"_bulk returned HTTP {status!r}, expected 200: {body!r}"
Issue-Key: opensearch.write-alias-rollover-routes-new-writes-to-new-backing-index|run#9|AssertionError|assert status == 200, f"adding the write alias returned HTTP {status!r}: {aliased!r}"
Issue-Key: opensearch.cluster-settings-put-is-refused-for-a-non-allowlisted-property|run#9|AssertionError|assert status == 200, (
Issue-Key: opensearch.disabled-refresh-interval-hides-document-from-search-until-refresh|run#9|AssertionError|assert status == 200, f"reading {index!r} settings returned HTTP {status!r}: {settings!r}"
Issue-Key: opensearch.osq-deep-opensearch-2-custom-routing-selects-the-shard-for-get-and-search|run#9|AssertionError|assert status == 200 and settings[index]["settings"]["index"]["number_of_shards"] == "2", (
Issue-Key: opensearch.index-mapping-conflict-rejected|run#23|AssertionError|assert _error_type(conflict) == "illegal_argument_exception", (
Issue-Key: opensearch.knn-vector-query-nearest-neighbour-ordering|run#9|AssertionError|assert status == 200, f"_mapping read failed with HTTP {status}: {mapping}"
Issue-Key: opensearch.nested-mapping-isolates-sub-documents-from-flattened-object-matching|run#8|AssertionError|assert status == 200, f"reading back the nested mapping failed with HTTP {status}: {mapping}"
Issue-Key: opensearch.custom-analyzer-token-stream-drives-search-match|run#8|AssertionError|assert status == 200, f"_analyze returned HTTP {status!r}: {analyzed!r}"
Issue-Key: opensearch.os-index-document-search-roundtrip|run#44|AssertionError|assert status == 200 and isinstance(rows, list), (
Issue-Key: opensearch.osx-deep-opensearch-3-blue-green-scale-out-data-continuity|run#7|AssertionError|assert status == 200, f"_cat/nodes before scaling returned HTTP {status}: {nodes_before!r}"
Issue-Key: opensearch.osq-deep-opensearch-2-template-priority-composition-and-equal-priority-overlap-rejected|run#11|AssertionError|assert status == 200 and body.get("acknowledged") is True, (
Issue-Key: opensearch.concurrent-conditional-writes-elect-exactly-one-winner|run#12|AssertionError|assert isinstance(seq_no, int), f"_seq_no missing on the seed response: {seed!r}"
Issue-Key: opensearch.document-optimistic-concurrency-version-conflict-rejected|run#12|AssertionError|assert isinstance(stale_seq_no, int), f"_seq_no missing on the index response: {first}"
Issue-Key: opensearch.osq-deep-opensearch-2-highlight-wraps-matched-term-on-text-but-not-keyword-sibling|_only_hit#13|AssertionError|assert _total(result) == 1 and [h["_id"] for h in hits] == ["h1"], (
Issue-Key: opensearch.sorting-and-aggregating-a-text-field-rejected-while-its-keyword-subfield-serves-both|run#13|AssertionError|assert sorted(_hit_titles(matched)) == SORTED_TITLES, (
Issue-Key: opensearch.osc-deep-opensearch-1-over-replicated-index-yellow-then-green|_index_health#7|Timeout|time.sleep(3.0)
Issue-Commit: 67f5fadccaf81bef3549b8820a70ca0b0ed0533e

# opensearch: the domain REST data plane serves only a narrow route set — _bulk, _aliases, _settings, _mapping, _analyze, _cat/* and _component_template are unrouted, document writes omit _seq_no/_primary_term, and a match on an analyzed text field finds nothing (10 causes)

> _Note: this issue was identified against the code base at commit `67f5fadccaf81bef3549b8820a70ca0b0ed0533e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DomainRestDataPlane

## Description
The domain REST data plane is now reachable (signed requests to DomainStatus$Endpoint no longer answer 404 UnknownService 'es', which is what open issue 20261029T182036Z-opensearch-the-domain-rest-data-plane-answers-404.md and 20261024T055421Z-opensearch-domain-data-plane-is-not-served-and.md describe), so these are net-new divergences of the surface that now answers: the router registers a bounded route set and the document/search layer diverges from the engine on three further points. Ten root causes, each with its own failing check ids.

Cause 1 of 10 — POST /_bulk is not served; the path is resolved as an INDEX name.
AWS: Amazon OpenSearch Service's documented data-plane surface includes the bulk API; `POST /_bulk` (and `POST /{index}/_bulk`) take a newline-delimited action/source body and answer HTTP 200 with `{"took":<n>,"errors":<bool>,"items":[...]}`, 200 even when individual items failed, each item carrying its own `status` and `error`. Recorded behavior only for the emulator's wording. The emulator refuses with the INDEX-level route's method list, which is the tell that `/_bulk` was matched as `/{index}`.
Emulator: HTTP 405 `{'error': 'Incorrect HTTP method for uri [/_bulk] and method [POST], allowed: [PUT, GET, HEAD, DELETE]'}`. Fixed text: "Incorrect HTTP method for uri [", "] and method [", "], allowed: ["; variable parts: the request URI (`/_bulk`), the request method (`POST`) and the allowed-method list — here the list belongs to the index route, not to `_bulk`.
Why: a client that loads documents with the bulk API — the normal ingest path for every OpenSearch SDK and for Logstash/Fluent Bit/opensearch-py helpers — cannot write a single document, and a `_`-prefixed reserved top-level endpoint is silently reinterpreted as an index name, so the refusal misdirects the caller.
Fix: the domain REST data-plane router in services/service-com-amazonaws-opensearch-amazonopensearchservice/src/service/domain_rest.rs — reserve `_`-prefixed first segments from index-name resolution and register the bulk handler against the document/ingest state.
Traces: opensearch.bm25-relevance-ordering-and-explain-score-arithmetic, opensearch.bulk-index-partial-failure-item-errors-rejected, opensearch.closed-index-search-rejected-and-reopen-restores-documents, opensearch.closed-point-in-time-reader-rejected, opensearch.cluster-health-and-shard-allocation-match-domain-config, opensearch.msearch-responses-are-positional-and-isolate-a-failing-slot, opensearch.osc-immutable-number-of-shards-update-rejected, opensearch.osq-deep-opensearch-2-async-delete-by-query-task-completes-with-exact-subset, opensearch.point-in-time-search-after-walk-isolated-from-concurrent-indexing, opensearch.reindex-into-keyword-mapping-preserves-documents-and-enables-term-match, opensearch.restore-over-open-index-rejected, opensearch.scroll-pagination-returns-each-document-exactly-once, opensearch.search-aggregation-query-bucket-ordering, opensearch.snapshot-restore-index-round-trip, opensearch.update-by-query-script-touches-exactly-the-matched-documents

Cause 2 of 10 — POST /_aliases is not served; the path is resolved as an INDEX name.
AWS: `POST /_aliases` takes `{"actions":[{"add":{"index":...,"alias":...,"is_write_index":true}}, ...]}` and answers HTTP 200 `{"acknowledged":true}`. Recorded behavior only for the emulator's wording.
Emulator: HTTP 405 `{'error': 'Incorrect HTTP method for uri [/_aliases] and method [POST], allowed: [PUT, GET, HEAD, DELETE]'}` — again the index route's method list.
Why: write aliases and rollover are how callers do zero-downtime index rotation; without the alias-actions API a client cannot point a write alias at a backing index at all.
Fix: the same data-plane router — register the alias-actions handler and keep the alias set in the domain's index state.
Traces: opensearch.write-alias-rollover-routes-new-writes-to-new-backing-index

Cause 3 of 10 — /{index}/_settings is unrouted for GET and PUT.
AWS: `GET /{index}/_settings` answers 200 with `{"<index>":{"settings":{"index":{"number_of_shards":"<n>","number_of_replicas":"<n>","refresh_interval":...}}}}` (values are strings); `PUT /{index}/_settings` answers 200 `{"acknowledged":true}` for a dynamic setting and refuses a static one. Recorded behavior only for the emulator's wording.
Emulator: HTTP 400 `{'error': 'no handler found for uri [/<index>/_settings] and method [GET]'}` and the same for `PUT`. Fixed text: "no handler found for uri [", "] and method ["; variable parts: the request URI (which carries the caller's index name) and the method.
Why: index settings are how a caller reads back shard/replica counts and turns refresh off; a 400 with no error type also breaks clients that branch on the engine's `error.type`.
Fix: the data-plane router plus the index record's settings in that crate's state.
Traces: opensearch.cluster-settings-put-is-refused-for-a-non-allowlisted-property, opensearch.disabled-refresh-interval-hides-document-from-search-until-refresh, opensearch.osq-deep-opensearch-2-custom-routing-selects-the-shard-for-get-and-search

Cause 4 of 10 — /{index}/_mapping is unrouted for GET and PUT.
AWS: `GET /{index}/_mapping` answers 200 with `{"<index>":{"mappings":{"properties":{...}}}}`; `PUT /{index}/_mapping` answers 200 `{"acknowledged":true}` for a compatible addition and refuses an incompatible type change with HTTP 400 and `error.type` `illegal_argument_exception`. Recorded behavior only for the emulator's wording.
Emulator: HTTP 400 `{'error': 'no handler found for uri [/<index>/_mapping] and method [GET]'}` (and `[PUT]`), with no `error.type` at all, so a caller asserting on the mapping-conflict class reads `''`.
Why: a client cannot read back or evolve a mapping, and the documented mapping-conflict refusal class is unobservable.
Fix: the data-plane router plus the index record's mapping in that crate's state.
Traces: opensearch.index-mapping-conflict-rejected, opensearch.knn-vector-query-nearest-neighbour-ordering, opensearch.nested-mapping-isolates-sub-documents-from-flattened-object-matching

Cause 5 of 10 — POST /{index}/_analyze is unrouted.
AWS: `POST /{index}/_analyze` answers 200 with `{"tokens":[{"token":...,"start_offset":...,"end_offset":...,"type":...,"position":...}, ...]}` for the index's configured analyzer. Recorded behavior only for the emulator's wording.
Emulator: HTTP 400 `{'error': 'no handler found for uri [/<index>/_analyze] and method [POST]'}`.
Why: `_analyze` is the only way a caller can verify that a custom analyzer produces the tokens their queries depend on; without it an analyzer misconfiguration is undiagnosable.
Fix: the data-plane router plus the analysis chain the index's settings declare.
Traces: opensearch.custom-analyzer-token-stream-drives-search-match

Cause 6 of 10 — the _cat APIs are unrouted.
AWS: `GET /_cat/indices?format=json` answers 200 with a JSON array, one object per index (`health`, `status`, `index`, `pri`, `rep`, `docs.count`, ...); `GET /_cat/nodes?format=json` answers 200 with one object per node. Recorded behavior only for the emulator's wording.
Emulator: HTTP 400 `{'error': 'no handler found for uri [/_cat/indices] and method [GET]'}` and `{'error': 'no handler found for uri [/_cat/nodes] and method [GET]'}`.
Why: `_cat` is the documented way to enumerate indexes and nodes; a caller that lists its indexes or checks that a scale-out added nodes gets a 400 instead of the listing.
Fix: the data-plane router, answering out of the domain's index records and its ClusterConfig topology.
Traces: opensearch.os-index-document-search-roundtrip, opensearch.osx-deep-opensearch-3-blue-green-scale-out-data-continuity

Cause 7 of 10 — PUT /_component_template/{name} is unrouted.
AWS: `PUT /_component_template/{name}` answers 200 `{"acknowledged":true}`; composable index templates then reference it through `composed_of`, and two index templates matching the same pattern at EQUAL priority are refused HTTP 400 `illegal_argument_exception`. Recorded behavior only for the emulator's wording.
Emulator: HTTP 400 `{'error': 'no handler found for uri [/_component_template/<name>] and method [PUT]'}`.
Why: composable templates are the supported template model for OpenSearch 2.x; a client cannot register a component template, so template composition and the equal-priority refusal are both unreachable.
Fix: the data-plane router plus the index-template state (`IndexTemplate`) in that crate.
Traces: opensearch.osq-deep-opensearch-2-template-priority-composition-and-equal-priority-overlap-rejected

Cause 8 of 10 — a document write response omits _seq_no and _primary_term.
AWS: an index/create/update response carries `_index`, `_id`, `_version`, `result`, `_shards`, AND `_seq_no` and `_primary_term`; those two are the values a caller feeds back as `if_seq_no`/`if_primary_term` to do an optimistic-concurrency compare-and-set. Recorded behavior only for the emulator's response shape.
Emulator: the write answers 200 with `{'_index': '<index>', '_id': '<id>', '_version': 1, 'result': 'created', '_shards': {'total': 1, 'successful': 1, 'failed': 0}}` — no `_seq_no`, no `_primary_term`.
Why: optimistic concurrency control is impossible: a client has nothing to pass as `if_seq_no`, so it cannot do a conditional write and cannot observe the documented `version_conflict_engine_exception` on a stale one. Two concurrent writers both win.
Fix: the document-write serializer and the per-document sequence/primary-term state in that crate's ingest state.
Traces: opensearch.concurrent-conditional-writes-elect-exactly-one-winner, opensearch.document-optimistic-concurrency-version-conflict-rejected

Cause 9 of 10 — a `match` query on an analyzed `text` field finds no documents.
AWS: a `text` field is analyzed at index time and a `match` query is analyzed the same way, so `{"query":{"match":{"<text field>":"<term>"}}}` returns every document whose analyzed field contains that term, with `hits.total.value` equal to that count. Recorded behavior only for the emulator's result.
Emulator: the search answers 200 but with an EMPTY hit set for documents that were indexed successfully — `match on text 'body': expected exactly the one indexed document, got total 0 ids []`, and `a match on the analysed text field must find all three documents; got []`.
Why: full-text search is the service's whole purpose; a caller that indexes documents and searches them with the documented `match` query gets a silent empty result rather than an error, which is worse than a refusal because nothing signals the failure.
Fix: the search/selector evaluation and the index-time analysis of `text` fields in the data plane (the `selector` query vocabulary in domain_rest and the index mapping's field types).
Traces: opensearch.osq-deep-opensearch-2-highlight-wraps-matched-term-on-text-but-not-keyword-sibling, opensearch.sorting-and-aggregating-a-text-field-rejected-while-its-keyword-subfield-serves-both

Cause 10 of 10 — a two-node domain's replica-allocation health walk never produces a response.
AWS: an index created with more replicas than there are data nodes reports cluster/index status `yellow` with unassigned replica shards, and after the replica count is lowered to fit the topology it reports `green`; both are observable on `GET /_cluster/health` within seconds. Recorded behavior only: the emulator produces no answer for this walk at all.
Emulator: the check produces no response and is cut off — `Check timed out after 60.0s` — so neither the `yellow` nor the `green` state is ever reported. This is the same family as open issue 20261027T004336Z-opensearch-ten-checks-never-produce-a-response-freshly.md, which that issue does not cover for this check. Hypothesis: the two-node domain never settles, or the health arithmetic never transitions, so the bounded poll runs to the harness cap.
Why: a client that waits for a documented state transition hangs instead of being told anything, burning its whole deadline.
Fix: the cluster-health computation in the data plane and the domain's post-create/post-update settling in that crate's state.
Traces: opensearch.osc-deep-opensearch-1-over-replicated-index-yellow-then-green

## Evidence
Cause 1: expected HTTP 200 bulk response; got HTTP 405 {'error': 'Incorrect HTTP method for uri [/_bulk] and method [POST], allowed: [PUT, GET, HEAD, DELETE]'} (15 checks).
Cause 2: expected HTTP 200 {'acknowledged': true}; got HTTP 405 {'error': 'Incorrect HTTP method for uri [/_aliases] and method [POST], allowed: [PUT, GET, HEAD, DELETE]'}.
Cause 3: expected HTTP 200 settings document; got HTTP 400 {'error': "no handler found for uri [/sim2real-behav-refresh/_settings] and method [GET]"} and {'error': 'no handler found for uri [/sim2real-behav-clustersettings/_settings] and method [PUT]'}.
Cause 4: expected HTTP 200 mapping document / HTTP 400 illegal_argument_exception on conflict; got HTTP 400 {'error': 'no handler found for uri [/sim2real-behav-mapping/_mapping] and method [PUT]'} with error type ''.
Cause 5: expected HTTP 200 {'tokens': [...]}; got HTTP 400 {'error': 'no handler found for uri [/sim2real-behav-analyzer/_analyze] and method [POST]'}.
Cause 6: expected HTTP 200 and a JSON list; got HTTP 400 {'error': 'no handler found for uri [/_cat/indices] and method [GET]'} and {'error': 'no handler found for uri [/_cat/nodes] and method [GET]'}.
Cause 7: expected HTTP 200 {'acknowledged': true}; got HTTP 400 {'error': 'no handler found for uri [/_component_template/<name>] and method [PUT]'}.
Cause 8: expected an index response carrying _seq_no and _primary_term; got {'_index': 'sim2real-behav-cas', '_id': 'counter', '_version': 1, 'result': 'created', '_shards': {'total': 1, 'successful': 1, 'failed': 0}}.
Cause 9: expected a match on a text field to return the indexed documents; got total 0 / ids [] while the documents exist.
Cause 10: expected a yellow-then-green cluster-health transition; the walk produced no answer and was cut off at the 60 s cap.

## Steps to Reproduce
1. CreateDomain with EngineVersion OpenSearch_2.11, ClusterConfig {InstanceType t3.small.search, InstanceCount 1}, EBSOptions gp3/10, and an AccessPolicies document allowing es:* for the caller; wait for the domain to be active and read DomainStatus.Endpoint.
2. Cause 1: SigV4-sign and send POST <endpoint>/_bulk with an NDJSON body of {"index":{"_index":...,"_id":...}} / source pairs -> 405 instead of 200.
3. Cause 2: POST <endpoint>/_aliases with {"actions":[{"add":{"index":"<idx>","alias":"<alias>","is_write_index":true}}]} -> 405 instead of 200.
4. Cause 3: PUT <endpoint>/<index> to create an index, then GET <endpoint>/<index>/_settings and PUT <endpoint>/<index>/_settings {"index":{"refresh_interval":"-1"}} -> 400 'no handler found'.
5. Cause 4: GET <endpoint>/<index>/_mapping, then PUT <endpoint>/<index>/_mapping with a conflicting field type -> 400 'no handler found' instead of the mapping / illegal_argument_exception.
6. Cause 5: POST <endpoint>/<index>/_analyze {"analyzer":"<custom>","text":"..."} -> 400 'no handler found'.
7. Cause 6: GET <endpoint>/_cat/indices?format=json and GET <endpoint>/_cat/nodes?format=json -> 400 'no handler found'.
8. Cause 7: PUT <endpoint>/_component_template/<name> {"template":{"settings":{...}}} -> 400 'no handler found'.
9. Cause 8: PUT <endpoint>/<index>/_doc/<id> {"n":1} and inspect the response body -> no _seq_no / _primary_term, so a follow-up PUT ...?if_seq_no=&if_primary_term= cannot be formed.
10. Cause 9: PUT <endpoint>/<index>/_doc/<id> with a text field, refresh, then POST <endpoint>/<index>/_search {"query":{"match":{"<text field>":"<term in the value>"}}} -> 200 with hits.total.value 0.
11. Cause 10: CreateDomain with InstanceCount 2, create an index with number_of_replicas greater than the node count, and poll GET <endpoint>/_cluster/health for yellow then (after lowering replicas) green -> no answer within the window.
