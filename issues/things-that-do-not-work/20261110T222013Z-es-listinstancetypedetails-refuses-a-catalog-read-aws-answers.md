Issue-Source: trace-sweep-divergence
Issue-Trace: opensearch-list-versions-and-instance-type-details-ignore-max-results
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: opensearch-list-versions-and-instance-type-details-ignore-max-results
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: es|ListInstanceTypeDetails|2xx->4xx|$.InstanceTypeDetails
Issue-Key: es|ListInstanceTypeDetails|2xx->4xx|status
Issue-Commit: 46b1ef78a2de21b94439ce78901879d208363fd1

# es ListInstanceTypeDetails refuses a catalog read AWS answers 200, returning HTTP 400 with no InstanceTypeDetails

> _Note: this issue was identified against the code base at commit `46b1ef78a2de21b94439ce78901879d208363fd1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstanceTypeDetails

## Description
ONE net-new cause on the es/opensearch instance-type catalog surface: a ListInstanceTypeDetails read that AWS answers 200 with an InstanceTypeDetails list is refused by the emulator with HTTP 400 and a body that carries no InstanceTypeDetails member at all.

Cause 1 of 1 — a valid GET /2021-01-01/opensearch/instanceTypeDetails/{EngineVersion} in the list-versions/instance-type-details flow is answered 400 instead of 200 (fixture opensearch-list-versions-and-instance-type-details-ignore-max-results, step task#6).
AWS: Smithy (services/service-com-amazonaws-opensearch-amazonopensearchservice) ListInstanceTypeDetails — http GET /2021-01-01/opensearch/instanceTypeDetails/{EngineVersion}, success code 200; input EngineVersion @httpLabel @required @length(14,18) @pattern ^Elasticsearch_[0-9]{1}\.[0-9]{1,2}$|^OpenSearch_[0-9]{1,2}\.[0-9]{1,2}$, plus the optional @httpQuery filters/paging members DomainName (domainName), MaxResults (maxResults, default 0, @range max 100), NextToken (nextToken), RetrieveAZs (retrieveAZs) and InstanceType (instanceType, @length(10,40) @pattern ^.*\..*\.search$); paginated{inputToken NextToken, outputToken NextToken, pageSize MaxResults}; modeled errors BaseException(400), ValidationException(400), ResourceNotFoundException(409), InternalException(500). A request whose EngineVersion satisfies that pattern and whose optional query members are within their modeled bounds is a well-formed catalog read, and recorded behavior shows AWS answering it 200 with an InstanceTypeDetails list (the recording's list for this step is headed by the entries for InstanceType 'c5.large.search' and 'c5.xlarge.search'); AWS does not refuse the read, and in particular an in-range maxResults is not an error — it is at most a page size. The modeled 400 classes are reserved for a request that actually violates a constraint (ValidationException carries AWS's '<n> validation error(s) detected: Value '<value>' at '<memberName>' failed to satisfy constraint: ...' template, whose fixed sentence frame is AWS's and whose value/member/constraint parts come from the request) — nothing in this request supplies one.
Emulator: answers the same read HTTP 400. The response body contains no InstanceTypeDetails member (replay reports `$.InstanceTypeDetails: missing (expected [{'InstanceType': 'c5.large.search'}, {'InstanceType': 'c5.xlarge.search'}], got None)`), so the whole catalog page is lost, not merely reordered or truncated. The step does not assert the error code or message the emulator emits, so only the status class — success turned into a client error — is established here; whatever error the handler raises, it is raised for a request that satisfies every modeled constraint. Hypothesis: the handler validates one of the optional query members (an in-range maxResults, a nextToken, or an instanceType/domainName filter) or an engine version its hard-coded catalog does not know, and refuses instead of serving the page.
Why: a client that sizes or upgrades a domain reads this endpoint to discover which instance types an engine version supports. Against the emulator the read fails outright with a client error, so the client cannot enumerate any instance type: SDK pagination raises instead of yielding a page, retry logic treats the 4xx as non-retryable and the sizing/upgrade workflow aborts — a strictly worse outcome than the already-filed catalog-content divergence, where at least a 200 page is returned.
Fix: the ListInstanceTypeDetails handler and its request validation in the es/opensearch service crate — a request whose EngineVersion matches the modeled pattern and whose optional query members (DomainName, MaxResults within @range max 100, NextToken, RetrieveAZs, InstanceType) are within their modeled bounds must be served 200 from the per-engine-version catalog data (paged, not refused); the 400 ValidationException path must fire only on an actual constraint violation and must then carry AWS's validation-error sentence with the offending value and member name taken from the request.
Traces: opensearch-list-versions-and-instance-type-details-ignore-max-results#task#6.

Not covered by any open issue: 20261103T112029Z-es-listinstancetypedetails-serves-a-stale-truncated-instance-type.md is about the CONTENT and ordering of the list on responses the emulator already answers 200 (expected_status 200 / actual_status 200) and carries no replay key for this step; this cluster is a 200 -> 400 status-class divergence and is reported by replay as un-owned.

## Evidence
Replay cluster (verdict=mismatch), divergence_key service=es|ListInstanceTypeDetails|status=200->400|keys=$.InstanceTypeDetails,status; affected_count 1.

Fixture opensearch-list-versions-and-instance-type-details-ignore-max-results, step task#6, operation ListInstanceTypeDetails, service es:
  expected_status 200, actual_status 400
  detail: "status: expected 200, got 400" and "$.InstanceTypeDetails: missing (expected [{'InstanceType': 'c5.large.search'}, {'InstanceType': 'c5.xlarge.search'}], got None)"

Expected: HTTP 200 with an InstanceTypeDetails list (the recorded page's entries include InstanceType 'c5.large.search' and 'c5.xlarge.search').
Actual: HTTP 400 with no InstanceTypeDetails member in the body. The step asserts no error code or message, so the error code/message the emulator emits is not established by this recording — the established defect is that a request satisfying every modeled input constraint is answered with a client-error status instead of the modeled 200.

Smithy cited: ListInstanceTypeDetails GET /2021-01-01/opensearch/instanceTypeDetails/{EngineVersion} code 200; EngineVersion @httpLabel @required @length(14,18) @pattern ^Elasticsearch_[0-9]{1}\.[0-9]{1,2}$|^OpenSearch_[0-9]{1,2}\.[0-9]{1,2}$; MaxResults @httpQuery maxResults default 0 @range(max 100); NextToken @httpQuery nextToken; RetrieveAZs @httpQuery retrieveAZs; InstanceType @httpQuery instanceType @length(10,40) @pattern ^.*\..*\.search$; DomainName @httpQuery domainName @length(3,28); paginated{inputToken NextToken, outputToken NextToken, pageSize MaxResults}; errors BaseException(400), ValidationException(400), ResourceNotFoundException(409), InternalException(500).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus simparity-wire-aws-v1.
2. Replay fixture opensearch-list-versions-and-instance-type-details-ignore-max-results up to step task#6 — the ListInstanceTypeDetails read (GET /2021-01-01/opensearch/instanceTypeDetails/<EngineVersion> with the step's optional query members).
3. Observe the emulator answers HTTP 400 with a body containing no InstanceTypeDetails member, where the recording is HTTP 200 with an InstanceTypeDetails list whose entries include 'c5.large.search' and 'c5.xlarge.search'.
4. After the fix, the same step must answer 200 and return the catalog page; re-replay that fixture/step to confirm.
