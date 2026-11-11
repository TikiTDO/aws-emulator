Issue-Source: trace-sweep-divergence
Issue-Trace: artifact-compliance-inquiry-basic-workflow
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: artifact-compliance-inquiry-basic-workflow
Issue-Shape-Id: com.amazonaws.artifact#Artifact
Issue-Key: artifact|ListComplianceInquiryQueries|2xx->2xx|$.queries[*]
Issue-Key: artifact|ListComplianceInquiryQueries|2xx->2xx|$.queries[*].citations[*].sourceLink
Issue-Commit: c3de02674d16064d9f5c8a9be8af17d3d848d40e

# artifact ListComplianceInquiryQueries omits the inquiry's query record and diverges on its citation sourceLinks

> _Note: this issue was identified against the code base at commit `c3de02674d16064d9f5c8a9be8af17d3d848d40e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListComplianceInquiryQueries

## Description
Cause 1 of 1: the compliance-inquiry query-history read does not serve the QuerySummary record the inquiry produced, and the citation entries it does serialize carry a sourceLink that disagrees with the citation they belong to.

AWS: Smithy: ListComplianceInquiryQueries is GET /v1/compliance-inquiry/listQueries with required httpQuery complianceInquiryId (pattern ^compliance-inquiry-[a-zA-Z0-9]{16}$), optional maxResults (range 1-300) and nextToken (length 1-2048); it is @paginated with items=queries. Its output ListComplianceInquiryQueriesResponse.queries targets QueriesList, a list of QuerySummary, whose required members are queryIdentifier (Integer), query (LongStringAttribute), status (QueryStatus: PROCESSING|COMPLETED|FAILED), statusMessage (QueryStatusMessage enum) and createdAt (TimestampAttribute, date-time), with optional response, reviewType (HUMAN|AI), citations (CitationList of Citation{sourceLabel, sourceContent, sourceLink}) and updatedResponseVersions. Recorded behavior (the model does not state the population rule, so this part is recorded behavior only): after an inquiry is started and a query against it is submitted, the recorded 200 response for that inquiry id lists that query as the first element of queries, carrying its query text, its response text, its status/statusMessage pair and a citations list in which every Citation carries sourceLabel, sourceContent and the sourceLink for that same source. The requirement is structural - one QuerySummary per query the inquiry holds, in the service's order, each citation's sourceLink being the link of the source its sourceLabel/sourceContent describe. The particular URLs, blog links and compliance-program prose in the recording are AWS catalog content from one run and are not themselves the requirement.

Emulator: the step is answered 200 (expected 200, actual 200), so the status and the envelope are right, but the body's queries list does not contain the first query at all, and the citation links of that query diverge. The replay reports detail `$.queries[0]: missing (expected {'citations': [...], ...}, got None)` and clusters the diverging JSON paths as `$.queries[0]`, `$.queries[0].citations[0].sourceLink`, `$.queries[0].citations[1].sourceLink`, `$.queries[0].citations[2].sourceLink`. So the handler returns a queries list that is short by the inquiry's own query, and where citation objects are produced their sourceLink member is not the link belonging to that citation (Hypothesis: the citations are synthesized independently of the sourceLabel/sourceContent they are attached to, or sourceLink is dropped/defaulted rather than carried through from the stored query).

Why: a client that starts a compliance inquiry, submits a query and then reads the query history back - the normal AWS Artifact compliance-inquiry flow, and the only way to poll a query from PROCESSING to COMPLETED - gets an empty or short queries list and therefore concludes the query it just submitted does not exist. Any polling loop keyed on queries[*].status never terminates, pagination over queries (inputToken/outputToken nextToken, items=queries) under-reports the collection, and a client that renders the answer's provenance shows citation links that point somewhere other than the source text next to them, which for a compliance-evidence API is a correctness problem and not cosmetic.

Fix: the ListComplianceInquiryQueries handler and the compliance-inquiry state it reads - the query records written when a query is submitted against an inquiry must be enumerable by that inquiry id (every QuerySummary, with the modeled required members populated), and the serializer must emit each Citation's sourceLink from the stored citation rather than regenerating it, so sourceLabel/sourceContent/sourceLink stay one consistent triple. Note that the open issues 20261109T144745Z-artifact-compliance-inquiry-responses-omit-the-empty-tags.md (empty tags map omitted on GetComplianceInquiryMetadata/ExportComplianceInquiry) and 20261110T162501Z-artifact-exportcomplianceinquiry-serves-a-presigned-url-for-an.md (export allowed while PROCESSING) touch the same feature but are different root causes and do not cover this one; the replay reports no owner for this cluster.

Traces: artifact-compliance-inquiry-basic-workflow#task#3 (ListComplianceInquiryQueries, verdict mismatch, expected 200 / actual 200, 1 affected step).

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200, 31 steps, 27 passed, 4 mismatched).

Cluster: service=artifact | operation=ListComplianceInquiryQueries | verdict=mismatch | expected_status=200 | actual_status=200 | fixture_id=artifact-compliance-inquiry-basic-workflow | step_key=task#3 | affected=[artifact-compliance-inquiry-basic-workflow#task#3] | stub_path="" | owned_by=[].

divergence_key: service=artifact|ListComplianceInquiryQueries|status=200->200|keys=$.queries[0],$.queries[0].citations[0].sourceLink,$.queries[0].citations[1].sourceLink,$.queries[0].citations[2].sourceLink

Expected (recorded): queries[0] is a QuerySummary for the inquiry's query, carrying its citations list, each Citation having sourceLabel, sourceContent and the matching sourceLink.
Actual (emulator): queries[0] is absent - replay detail `$.queries[0]: missing (expected {...}, got None)` - and the citation sourceLink values at citations[0..2] of that query do not match the recorded citation links.

Model reference: services/service-com-amazonaws-artifact-artifact/smithy-model/com-amazonaws-artifact-artifact.json - ListComplianceInquiryQueries (GET /v1/compliance-inquiry/listQueries, @paginated items=queries), ListComplianceInquiryQueriesResponse.queries -> QueriesList -> QuerySummary (required queryIdentifier/query/status/statusMessage/createdAt; optional response/reviewType/citations/updatedResponseVersions), CitationList -> Citation{sourceLabel: ShortStringAttribute, sourceContent: LongStringAttribute, sourceLink: LongStringAttribute}.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire fixture corpus simparity-wire-aws-v1.
2. Replay fixture artifact-compliance-inquiry-basic-workflow and stop at step task#3.
   Equivalently, by hand against the emulator's artifact endpoint:
   a. StartComplianceInquiry for a service/compliance topic, capture the returned complianceInquiryId (compliance-inquiry-<16 alnum>).
   b. Submit the inquiry's query and let it settle (status COMPLETED).
   c. GET /v1/compliance-inquiry/listQueries?complianceInquiryId=<that id>
3. Observe the 200 body: the queries list does not contain a QuerySummary for the query submitted in step b (replay: `$.queries[0]: missing ... got None`), where the recording returns it as queries[0].
4. For a query whose citations the emulator does serialize, compare each Citation's sourceLink against the sourceLabel/sourceContent it is paired with: citations[0..2].sourceLink diverge from the recorded links for those same sources.
