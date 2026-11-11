Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.search-uri-query-exists-multi-match-and-date-field
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.search-uri-query-exists-multi-match-and-date-field|run#6|AssertionError|assert status == 201 and indexed.get("result") == "created", (
Issue-Commit: 552123697b202fc6b750c729592b678c1a752764

# opensearch: the domain data plane refuses a `date`-mapped field given a yyyy-MM-dd string — PUT /{index}/_doc/{id} answers 400 mapper_parsing_exception instead of 201 "created"

> _Note: this issue was identified against the code base at commit `552123697b202fc6b750c729592b678c1a752764`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PUT /{index}/_doc/{id} (domain REST data plane: index document)

## Description
Cause 1 of 1: the domain REST data plane's value coercion for a field mapped `{"type":"date"}` accepts only epoch-millis and refuses the date-only / `strict_date_optional_time` string form, so a document carrying a plain `yyyy-MM-dd` date cannot be indexed at all.

AWS: recorded behavior only for the wire shape — the Smithy model for this service (AmazonOpenSearchService 2021-01-01, restJson1, endpointPrefix/arnNamespace `es`, 96 operations) models only the `es` control plane (CreateDomain, DescribeDomain, …); the domain REST data plane reached at `DomainStatus.Endpoint` is not in the model. The engine behavior it must reproduce is OpenSearch's documented default for the `date` field type: its format is `strict_date_optional_time||epoch_millis`, i.e. a `date` field accepts BOTH an ISO-8601 string whose time part is optional (`2026-01-01`, `2026-02-15`) and an epoch-millis number (`1767225599000`). Indexing a document whose `date` field holds either form succeeds: `PUT /<index>/_doc/<id>` answers HTTP 201 with a body whose `result` is `"created"` (HTTP 200 / `"updated"` when the id already exists), and `GET /<index>/_doc/<id>` returns `_source` verbatim — the stored `_source` keeps the string exactly as sent, it is not rewritten to millis. A subsequent `range` query on that field compares instants, so `{"range":{"published":{"gte":"2026-01-01"}}}` matches both the string-dated documents, and the URI form `?q=published:2026-01-01` matches the document on that day.

Emulator: `PUT /<index>/_doc/d0` with body `{"title":"red apple","body":"fresh fruit","published":"2026-01-01","rating":5}`, against an index created with mapping `"published": {"type": "date"}`, answers HTTP 400 with
`{"error": {"root_cause": [{"type": "mapper_parsing_exception", "reason": "failed to parse field [published] of type [date] in document with id 'd0'. Preview of field's value: '2026-01-01'"}], "type": "mapper_parsing_exception", "reason": "failed to parse field [published] of type [date] in document with id 'd0'. Preview of field's value: '2026-01-01'"}, "status": 400}`.
The envelope and wording are the right ones for a value that genuinely cannot be parsed (fixed text: `failed to parse field [`, `] of type [`, `] in document with id '`, `'. Preview of field's value: '`, `'`; variable parts taken from the request: the field name `published`, the mapped type `date`, the document id, and the offending value). The defect is not the message template — it is that this refusal is produced for a value AWS parses. The emulator only accepts the epoch-millis form (the sibling document whose `published` is the number `1767225599000` is not what fails first), so the accepted-format set is narrower than `strict_date_optional_time||epoch_millis`.

Why: a client that maps a field as `date` and writes ISO dates — the overwhelmingly common shape for log/document ingestion, CloudFormation-driven index templates, Firehose/Lambda indexers and any ORM-ish serializer that emits `YYYY-MM-DD` or `YYYY-MM-DDTHH:MM:SSZ` — has every write rejected with a 400 it cannot act on, because the value IS valid for the mapping it declared. Nothing downstream can run: no `exists`, `multi_match`, `range` or URI-`q` query over that field is reachable, and a retry does not help since the request is deterministically refused. Code written against AWS that treats `mapper_parsing_exception` as "the document is malformed, drop it to a dead-letter destination" will silently discard well-formed data.

Fix: the domain REST data-plane document-index handler's mapping-driven field-value coercion in the opensearch service crate — the `date` field type's value parser must accept the default `strict_date_optional_time||epoch_millis` format set (ISO-8601 with optional time/offset, plus epoch millis) and only raise `mapper_parsing_exception` for values outside it, while the stored `_source` continues to round-trip the value exactly as supplied. The same parser feeds the `range` comparison and the URI-query term resolution for a `date` field, so those paths must read the coerced instant rather than the raw token.

Traces: opensearch.search-uri-query-exists-multi-match-and-date-field (asserts at the index step, before any search assertion is reached).

## Evidence
Check: opensearch.search-uri-query-exists-multi-match-and-date-field [mismatch]

Setup (from the check): a minimal single-node OpenSearch_2.11 domain, then `PUT /<index>` with mapping
  {"title": {"type": "text"}, "body": {"type": "text"}, "published": {"type": "date"}, "rating": {"type": "integer"}}
(HTTP 200 — index creation succeeded).

Request that diverges: `PUT /<index>/_doc/d0` (SigV4 'es'), body
  {"title": "red apple", "body": "fresh fruit", "published": "2026-01-01", "rating": 5}

Expected (AWS / OpenSearch default `date` format `strict_date_optional_time||epoch_millis`):
  HTTP 201, body with "result": "created"

Actual (emulator):
  HTTP 400, body:
  {"error": {"root_cause": [{"type": "mapper_parsing_exception", "reason": "failed to parse field [published] of type [date] in document with id 'd0'. Preview of field's value: '2026-01-01'"}], "type": "mapper_parsing_exception", "reason": "failed to parse field [published] of type [date] in document with id 'd0'. Preview of field's value: '2026-01-01'"}, "status": 400}

Harness assertion:
  check_opensearch_search_uri_query_exists_multi_match_and_date_field.py:245
  AssertionError: indexing d0 with published='2026-01-01' must be created (HTTP 201), got HTTP 400: {...mapper_parsing_exception...}

Unreached consequences in the same check (all blocked by the 400 above): GET /<index>/_doc/d0 `_source` verbatim round-trip; `?q=title:apple`; `?q=red` over all fields; `{"exists":{"field":"rating"}}`; `{"multi_match":{"query":"red","fields":["title","body"]}}`; `{"range":{"published":{"gte":"2026-01-01"}}}`; `?q=published:2026-01-01`.

Note on scope: the second document in the fixture uses the epoch-millis form (`"published": 1767225599000`) and the string-dated one is what fails, which localizes the defect to the string branch of the `date` parser rather than to date handling as a whole.

## Steps to Reproduce
1. CreateDomain with a minimal single-node topology (EngineVersion OpenSearch_2.11, t3.small.search x1, DedicatedMasterEnabled=false, ZoneAwarenessEnabled=false, gp3/10GiB EBS) and an account-scoped AccessPolicies document allowing es:* on the domain.
2. Wait for DescribeDomain to report Processing=false with a published Endpoint; take that endpoint as the data-plane base URL.
3. SigV4-sign ('es') and send:
   PUT <base>/<index>
   {"settings": {"index": {"number_of_shards": 1, "number_of_replicas": 0}},
    "mappings": {"properties": {"title": {"type": "text"}, "body": {"type": "text"},
                                "published": {"type": "date"}, "rating": {"type": "integer"}}}}
   -> HTTP 200.
4. SigV4-sign ('es') and send:
   PUT <base>/<index>/_doc/d0
   {"title": "red apple", "body": "fresh fruit", "published": "2026-01-01", "rating": 5}
   Observed: HTTP 400 mapper_parsing_exception "failed to parse field [published] of type [date] ... Preview of field's value: '2026-01-01'".
   Expected: HTTP 201 with "result": "created".
5. Contrast: send the same PUT with "published": 1767225599000 (epoch millis) — accepted — showing only the ISO/date-only string form is refused.
6. After the fix, also confirm: GET <base>/<index>/_doc/d0 returns _source identical to the document as sent; POST <base>/<index>/_search with {"query": {"range": {"published": {"gte": "2026-01-01"}}}} matches the string-dated documents; GET <base>/<index>/_search?q=published:2026-01-01 matches the document dated that day.
7. Re-replay opensearch.search-uri-query-exists-multi-match-and-date-field.
