Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.closed-index-search-rejected-and-reopen-restores-documents opensearch.os-index-document-search-roundtrip opensearch.osc-immutable-number-of-shards-update-rejected opensearch.closed-point-in-time-reader-rejected opensearch.cluster-health-and-shard-allocation-match-domain-config opensearch.msearch-responses-are-positional-and-isolate-a-failing-slot opensearch.osc-deep-opensearch-1-over-replicated-index-yellow-then-green opensearch.osq-deep-opensearch-2-async-delete-by-query-task-completes-with-exact-subset opensearch.osq-deep-opensearch-2-custom-routing-selects-the-shard-for-get-and-search opensearch.reindex-into-keyword-mapping-preserves-documents-and-enables-term-match opensearch.restore-over-open-index-rejected opensearch.snapshot-restore-index-round-trip opensearch.write-alias-rollover-routes-new-writes-to-new-backing-index
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.closed-index-search-rejected-and-reopen-restores-documents|run#17|AssertionError|assert status == 200, f"_close returned HTTP {status!r}: {close_body!r}"
Issue-Key: opensearch.os-index-document-search-roundtrip|run#48|AssertionError|assert status == 404 and isinstance(err, dict) and err.get("type") == "index_not_found_exception", (
Issue-Key: opensearch.osc-immutable-number-of-shards-update-rejected|run#20|AssertionError|assert status == 200 and body.get("acknowledged") is True, (
Issue-Key: opensearch.closed-point-in-time-reader-rejected|_open_pit#9|AssertionError|assert status == 200, f"opening a point-in-time reader returned HTTP {status!r}: {body!r}"
Issue-Key: opensearch.cluster-health-and-shard-allocation-match-domain-config|run#20|AssertionError|assert status == 200, f"_cat/shards returned HTTP {status!r}: {shards!r}"
Issue-Key: opensearch.msearch-responses-are-positional-and-isolate-a-failing-slot|run#10|AssertionError|assert status == 200, (
Issue-Key: opensearch.osc-deep-opensearch-1-over-replicated-index-yellow-then-green|run#25|AssertionError|assert status == 200, f"_cluster/allocation/explain returned HTTP {status!r}: {explain!r}"
Issue-Key: opensearch.osq-deep-opensearch-2-async-delete-by-query-task-completes-with-exact-subset|run#15|AssertionError|assert status == 200, f"async _delete_by_query returned HTTP {status!r}: {accepted!r}"
Issue-Key: opensearch.osq-deep-opensearch-2-custom-routing-selects-the-shard-for-get-and-search|_shard_for#10|AssertionError|assert status == 200, f"_search_shards?routing={routing} returned HTTP {status!r}: {body!r}"
Issue-Key: opensearch.reindex-into-keyword-mapping-preserves-documents-and-enables-term-match|run#15|AssertionError|assert status == 200, f"_reindex returned HTTP {status!r}: {result!r}"
Issue-Key: opensearch.restore-over-open-index-rejected|_register_repo#14|AssertionError|raise AssertionError(
Issue-Key: opensearch.snapshot-restore-index-round-trip|_register_repo#13|AssertionError|raise AssertionError(
Issue-Key: opensearch.write-alias-rollover-routes-new-writes-to-new-backing-index|run#16|AssertionError|assert status == 200, f"_rollover returned HTTP {status!r}: {rollover!r}"
Issue-Commit: 181b4db8a49f9f99cf56c1b2f4aacf19bf82375e

# opensearch: ten domain REST data-plane endpoints are unrouted — _close/_open, point-in-time, _cat/shards, _msearch, allocation/explain, _delete_by_query, _search_shards, _reindex, _snapshot and _rollover all answer 400 'no handler found' (10 causes)

> _Note: this issue was identified against the code base at commit `181b4db8a49f9f99cf56c1b2f4aacf19bf82375e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
OpenSearch domain REST data plane (es:ESHttpGet / es:ESHttpPost / es:ESHttpPut)

## Description
Ten distinct OpenSearch REST endpoints on a created domain's published DomainStatus.Endpoint are not routed by the emulator's data plane. Each one answers the same emulator-authored envelope instead of the endpoint's real response:

    HTTP 400
    {"error": "no handler found for uri [<path>] and method [<METHOD>]"}

The fixed text of that envelope is `no handler found for uri [` ... `] and method [` ... `]`; `<path>` and `<METHOD>` are the request's own path and HTTP method. This is a routing gap, not a per-request value: the response does not depend on the body or on cluster state. Each cause below is a SEPARATE missing route and must be closed separately; the shared envelope is only the symptom they have in common. Ten causes.

Note: open issues 20261104T070924Z-opensearch-4-net-new-causes-the-domain-data.md (no _cat/indices handler) and 20261107T173234Z-opensearch-index-scoped-analyze-is-unrouted-a-custom.md (index-scoped _analyze unrouted) describe the SAME class of gap for DIFFERENT endpoints; neither covers any of the ten routes below, and the replay marks none of these checks as owned.

--- Cause 1 of 10: POST /<index>/_close and POST /<index>/_open are unrouted ---
AWS: recorded behavior only (the OpenSearch REST index close/open API is not part of the opensearch Smithy model; it reaches the domain through es:ESHttpPost). The recordings show POST /<index>/_close on an existing open index answering HTTP 200 with body `{"acknowledged": true, ...}`, after which a search against that index is refused and POST /<index>/_open restores it with HTTP 200 `{"acknowledged": true}` and the documents searchable again. On an index that does not exist, POST /<index>/_close answers HTTP 404 with an `index_not_found_exception` whose fixed reason text is `no such index [` + the requested index name + `]`.
Emulator: HTTP 400 `{"error": "no handler found for uri [/<index>/_close] and method [POST]"}` for every close, both for an existing index and for a deleted one.
Why: a client cannot close an index at all. Every workflow that depends on the closed state — refusing search on a closed index, applying an immutable setting such as number_of_shards while closed, and the 404 index_not_found_exception a close of a deleted index must produce — is unreachable, and the 400 is misread as a malformed request rather than as 'index missing' or 'not supported'.
Fix: the OpenSearch domain data plane's REST router and its index-state handler — add the _close/_open routes and the open/closed index state they toggle, including search refusal while closed and the 404 index_not_found_exception for an unknown index.
Traces: opensearch.closed-index-search-rejected-and-reopen-restores-documents, opensearch.os-index-document-search-roundtrip, opensearch.osc-immutable-number-of-shards-update-rejected.

--- Cause 2 of 10: POST /<index>/_search/point_in_time is unrouted ---
AWS: recorded behavior only. Opening a point-in-time reader with POST /<index>/_search/point_in_time?keep_alive=<duration> answers HTTP 200 with a body carrying a `pit_id` string that subsequent _search calls use, and DELETE /_search/point_in_time closes it; a search naming a closed/unknown pit_id is then refused.
Emulator: HTTP 400 `{"error": "no handler found for uri [/<index>/_search/point_in_time] and method [POST]"}`, so no reader is ever minted.
Why: point-in-time paging is unusable, and the negative class this guards (a search against an already-closed reader must be refused, not silently served) can never be observed.
Fix: the domain data plane's REST router plus point-in-time reader state (open/close/expire) in the search handler.
Traces: opensearch.closed-point-in-time-reader-rejected.

--- Cause 3 of 10: GET /_cat/shards and GET /_cat/shards/<index> are unrouted ---
AWS: recorded behavior only. GET /_cat/shards/<index> answers HTTP 200 with the per-shard allocation table (one row per primary and replica shard, with its state and node), matching the shard/replica counts the index settings and the domain's ClusterConfig imply.
Emulator: HTTP 400 `{"error": "no handler found for uri [/_cat/shards/<index>] and method [GET]"}`.
Why: shard allocation is unobservable, so a client cannot confirm that an index's primaries and replicas were placed as its domain topology allows, and cannot distinguish an unallocated replica from a healthy cluster.
Fix: the domain data plane's _cat route family and the shard-allocation view over index/cluster state.
Traces: opensearch.cluster-health-and-shard-allocation-match-domain-config.

--- Cause 4 of 10: POST /_msearch is unrouted ---
AWS: recorded behavior only. POST /_msearch with an NDJSON body answers overall HTTP 200 and a `responses` array that is POSITIONAL: slot i holds the result of request i, a slot that fails carries its own `error`/`status` while the sibling slots still carry their hits, and a per-slot failure never fails the whole multi-search.
Emulator: HTTP 400 `{"error": "no handler found for uri [/_msearch] and method [POST]"}` for the whole request.
Why: a client batching searches gets a single top-level 400 instead of per-slot results, so it cannot tell which slot failed and loses the results of the slots that succeeded — and it wrongly concludes the whole batch was malformed.
Fix: the domain data plane's REST router plus an _msearch handler that fans the NDJSON body out to the search handler and assembles a positional `responses` array with per-slot error isolation.
Traces: opensearch.msearch-responses-are-positional-and-isolate-a-failing-slot.

--- Cause 5 of 10: GET /_cluster/allocation/explain is unrouted ---
AWS: recorded behavior only. GET /_cluster/allocation/explain answers HTTP 200 with the allocation explanation for an unassigned shard (the shard identity, `current_state`, and the decision/decider detail explaining why it cannot be allocated) — which is how an over-replicated index's YELLOW health is diagnosed.
Emulator: HTTP 400 `{"error": "no handler found for uri [/_cluster/allocation/explain] and method [GET]"}`.
Why: a client cannot diagnose why a cluster is YELLOW; the standard explain call looks like a bad request rather than returning the unassigned-shard decision.
Fix: the domain data plane's _cluster route family and an allocation-explain view over shard state.
Traces: opensearch.osc-deep-opensearch-1-over-replicated-index-yellow-then-green.

--- Cause 6 of 10: POST /<index>/_delete_by_query is unrouted ---
AWS: recorded behavior only. POST /<index>/_delete_by_query answers HTTP 200; with wait_for_completion=false it returns a body carrying a `task` id that GET /_tasks/<task> then reports as completed, and exactly the documents matching the query are deleted while non-matching documents survive.
Emulator: HTTP 400 `{"error": "no handler found for uri [/<index>/_delete_by_query] and method [POST]"}`.
Why: query-scoped bulk deletion is impossible, and the asynchronous task handle AWS hands back (and the exact-subset guarantee it carries) cannot be exercised at all.
Fix: the domain data plane's REST router plus a _delete_by_query handler and the task record it returns when wait_for_completion=false.
Traces: opensearch.osq-deep-opensearch-2-async-delete-by-query-task-completes-with-exact-subset.

--- Cause 7 of 10: GET /<index>/_search_shards is unrouted ---
AWS: recorded behavior only. GET /<index>/_search_shards?routing=<value> answers HTTP 200 and names exactly the shard(s) the routing value hashes to, which is how custom routing is shown to pin a document to one shard for both get and search.
Emulator: HTTP 400 `{"error": "no handler found for uri [/<index>/_search_shards] and method [GET]"}`.
Why: a client cannot discover which shard a routing value selects, so custom-routing correctness is unverifiable and shard-targeted reads cannot be planned.
Fix: the domain data plane's REST router plus a _search_shards handler over the index's routing/shard map.
Traces: opensearch.osq-deep-opensearch-2-custom-routing-selects-the-shard-for-get-and-search.

--- Cause 8 of 10: POST /_reindex is unrouted ---
AWS: recorded behavior only. POST /_reindex with `{"source":{"index":<src>},"dest":{"index":<dst>}}` answers HTTP 200 with the reindex report (`total`, `created`, `failures`), copying every source document into the destination so the destination's own mapping (e.g. a keyword field) then governs matching.
Emulator: HTTP 400 `{"error": "no handler found for uri [/_reindex] and method [POST]"}`.
Why: the standard way to migrate documents into a corrected mapping does not exist, so a client cannot re-map an index without re-ingesting from its own source of truth.
Fix: the domain data plane's REST router plus a _reindex handler that copies documents between indices and re-applies the destination mapping.
Traces: opensearch.reindex-into-keyword-mapping-preserves-documents-and-enables-term-match.

--- Cause 9 of 10: PUT /_snapshot/<repo> and the snapshot/restore route family are unrouted ---
AWS: recorded behavior only. PUT /_snapshot/<repo> with an `s3` repository body (bucket, base_path, role_arn) answers HTTP 200 `{"acknowledged": true}` once the IAM role has propagated — and while propagation is pending it is refused with a repository-verification error, never a routing error. Snapshot create/restore/delete under /_snapshot/<repo>/<snapshot> then behave accordingly, and a restore over a still-open index is refused.
Emulator: HTTP 400 `{"error": "no handler found for uri [/_snapshot/<repo>] and method [PUT]"}`, which is not an IAM-propagation refusal and is not retryable, so no repository can ever be registered.
Why: snapshot and restore are entirely unavailable, and the 400 is indistinguishable from a malformed repository body — a client retrying on the propagation error retries forever.
Fix: the domain data plane's _snapshot route family — repository registration/verification plus snapshot create/restore/delete and the refusal for restoring over an open index.
Traces: opensearch.restore-over-open-index-rejected, opensearch.snapshot-restore-index-round-trip.

--- Cause 10 of 10: POST /<index>/_rollover is unrouted ---
AWS: recorded behavior only. POST /<alias-or-index>/_rollover answers HTTP 200 with `old_index`, `new_index` and `rolled_over: true`, and afterwards writes through the write alias land in the NEW backing index while the old one stays readable.
Emulator: HTTP 400 `{"error": "no handler found for uri [/<index>/_rollover] and method [POST]"}`.
Why: rollover-based index management is impossible; a write alias can never be advanced, so time/size-based index rotation cannot be modelled and writes keep landing in the original index.
Fix: the domain data plane's REST router plus a _rollover handler and the write-alias retargeting it performs.
Traces: opensearch.write-alias-rollover-routes-new-writes-to-new-backing-index.

## Evidence
All ten causes produce the same emulator envelope, differing only in path/method.

Expected (AWS): the endpoint's own response — e.g. HTTP 200 {"acknowledged": true} for POST /<index>/_close, HTTP 200 with a pit_id for POST /<index>/_search/point_in_time, HTTP 200 with a positional `responses` array for POST /_msearch, HTTP 200 {"acknowledged": true} for PUT /_snapshot/<repo>, HTTP 404 index_not_found_exception with reason `no such index [<index>]` for a _close of a deleted index.

Actual (emulator), verbatim from the replay:
- 'POST _close on deleted index expected 404 index_not_found_exception, got HTTP 400: {\'error\': \'no handler found for uri [/<index>/_close] and method [POST]\'}'
- '_close returned HTTP 400: {\'error\': \'no handler found for uri [/<index>/_close] and method [POST]\'}'
- 'opening a point-in-time reader returned HTTP 400: {\'error\': \'no handler found for uri [/<index>/_search/point_in_time] and method [POST]\'}'
- '_cat/shards returned HTTP 400: {\'error\': \'no handler found for uri [/_cat/shards/<index>] and method [GET]\'}'
- '_msearch with one failing slot returned overall HTTP 400, expected 200: {\'error\': \'no handler found for uri [/_msearch] and method [POST]\'}'
- '_cluster/allocation/explain returned HTTP 400: {\'error\': \'no handler found for uri [/_cluster/allocation/explain] and method [GET]\'}'
- 'async _delete_by_query returned HTTP 400: {\'error\': \'no handler found for uri [/<index>/_delete_by_query] and method [POST]\'}'
- '_search_shards?routing=k returned HTTP 400: {\'error\': \'no handler found for uri [/<index>/_search_shards] and method [GET]\'}'
- '_reindex returned HTTP 400: {\'error\': \'no handler found for uri [/_reindex] and method [POST]\'}'
- 'registering the s3 snapshot repository was refused with HTTP 400, which is not the IAM-propagation refusal this retry is for: {\'error\': \'no handler found for uri [/_snapshot/<repo>] and method [PUT]\'}'
- '_rollover returned HTTP 400: {\'error\': \'no handler found for uri [/<index>/_rollover] and method [POST]\'}'

Index, repository and domain names in the above are per-run values generated by the harness; they are not part of the requirement.

## Steps to Reproduce
1. CreateDomain with the minimal single-node topology (InstanceType t3.small.search, InstanceCount 1, DedicatedMasterEnabled false, ZoneAwarenessEnabled false, EBS gp3 10 GiB), EngineVersion OpenSearch_2.11, and wait for Processing=false.
2. Read DomainStatus.Endpoint and create an index with a few documents through the data plane.
3. Against that endpoint, issue each of these SigV4-signed requests and observe HTTP 400 {"error":"no handler found for uri [...] and method [...]"} instead of the endpoint's real response:
   - POST /<index>/_close  (then POST /<index>/_open)
   - POST /<index>/_search/point_in_time?keep_alive=1m
   - GET  /_cat/shards/<index>?format=json
   - POST /_msearch  (NDJSON body with one valid and one failing slot)
   - GET  /_cluster/allocation/explain
   - POST /<index>/_delete_by_query?wait_for_completion=false
   - GET  /<index>/_search_shards?routing=k
   - POST /_reindex
   - PUT  /_snapshot/<repo>  (type s3, with bucket/base_path/role_arn)
   - POST /<index>/_rollover
4. Also DELETE the index and re-issue POST /<index>/_close: expect HTTP 404 index_not_found_exception with reason `no such index [<index>]`, observe the same 400 routing envelope.
