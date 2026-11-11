Issue-Source: trace-sweep-divergence
Issue-Trace: artifact-catalog-list-reports
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: artifact-catalog-list-reports artifact-list-pagination artifact-compliance-inquiry-basic-workflow
Issue-Shape-Id: com.amazonaws.artifact#Artifact
Issue-Key: artifact|ListReports|2xx->2xx|$.reports[arn="arn
Issue-Key: artifact|ListComplianceInquiryQueries|2xx->2xx|$.queries[*]
Issue-Commit: 8d9be40717a60d4ab4e72e06403011e9fbdef268

# artifact list operations answer 200 with the recorded resources absent

## Resolution (partial: Cause 2 fixed, Cause 1 declined)

Cause 2 (ListComplianceInquiryQueries) is FIXED. A compliance-inquiry query is now answered
out of the published report catalog this service serves: the documents whose text carries
the query's terms are the sources, the matched passage of each one is what it contributes,
and the query reports that response together with the citations naming, quoting and linking
to those sources, once the clock-owed transition has made its processing due. The answer
plane lives in `services/service-com-amazonaws-artifact-artifact/src/state/inquiry_answers.rs`
(retrieval by the shared `services/common/src/document_text_search.rs`), is projected by
`ListComplianceInquiryQueries` and by the exported document, and is covered by
`tests/all/inquiry_query_answers.rs`. The remaining replay divergence on this key is the
answer's WORDING — an AWS answer is model-generated text and no such engine runs in-process —
and the three `citations[].sourceLink` values, which must name this emulator's own endpoint
and can never equal AWS's URLs.

Cause 1 (ListReports serving a short report catalog) is DECLINED: it can only be satisfied by
fitting the recorded instance. The replay reports 413 distinct `$.reports[arn="…"]`
divergences for it — the recording's catalog holds several hundred reports under
AWS-assigned ids whose names, descriptions, series and period bounds the replay withholds and
no AWS rule produces (the issue itself states the membership is recorded behaviour only). The
rule it states — every published report appears in exactly one page of the enumeration, each
entry carrying the full `ReportSummary` keyed by its `<reportId>:<version>` arn — is already
implemented (`src/ops/list_reports.rs`, `Report::to_summary`) and pinned by
`tests/all/pagination_page_walk.rs` and `tests/all/report_projection.rs`. The divergence key
`artifact|ListReports|2xx->2xx|$.reports[arn="arn` therefore remains open as recorded-catalog
membership, not as a code defect.

> _Note: this issue was identified against the code base at commit `8d9be40717a60d4ab4e72e06403011e9fbdef268`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListReports

## Description
Two net-new root causes in the artifact READ plane. Both are paginated list operations that answer HTTP 200 with a well-formed envelope whose collection does not contain the resources the service is supposed to be holding, so a client sees a successful, authoritative-looking empty/short page instead of the catalog or the query history. Each cause is independent and must be closed separately.

Cause 1 of 2 — ListReports serves a report catalog that is missing published reports (fixture artifact-catalog-list-reports, step task#1; same cause also observed on the pagination fixture's second page).
AWS: ListReports (Smithy: restJson1 GET /v1/report/list, http code 200; input maxResults @range 1..300 @httpQuery, nextToken @length 1..2048 @httpQuery; output reports: ReportsList, nextToken; @paginated inputToken=nextToken outputToken=nextToken items=reports pageSize=maxResults; @readonly; errors AccessDeniedException 403, ValidationException 400, ResourceNotFoundException 404, ServiceQuotaExceededException 402, ThrottlingException 429, InternalServerException 500). ListReports enumerates the service's whole report catalog, one page at a time, as ReportSummary entries (Smithy ReportSummary: id @pattern ^report-[a-zA-Z0-9]{16}$, arn, name, series, category, companyName, productName, description, state: PublishedState, version @range min 1, periodStart/periodEnd @timestampFormat date-time, acceptanceType, uploadState, statusMessage). Every PUBLISHED, PUBLIC-visibility report in the catalog must appear in exactly one page of the enumeration, and each entry carries the full ReportSummary for that report, keyed by its arn (the report id plus the ':<version>' suffix). Nothing in the model bounds which reports exist, so the catalog's membership is recorded behavior only: the recording shows the enumeration containing PASSTHROUGH-acceptance, PUBLISHED, PUBLIC reports across several series (an accessibility-conformance ACR series, a national information-assurance assessment series, and more), each with its own arn/id/version/series/category/description and period bounds.
Emulator: HTTP 200, but entries the enumeration must contain are absent from $.reports — the replay reports, per missing report, '$.reports[arn="<report arn>"]: missing (expected {...full ReportSummary...}, got None)'. No status or error divergence and no extra entries are reported: the envelope is right and the catalog behind it is short, so the pages it does serve enumerate fewer reports than the service holds. The same shortfall shows up on a later page of the paginated enumeration, i.e. it is the catalog's membership that is incomplete, not one page's slicing.
Why: ListReports is the only discovery path for AWS Artifact's reports — a client enumerates it to find the report it then accepts terms for and downloads. A successful 200 whose page simply omits a report is indistinguishable from that report not existing, so the client concludes the compliance document is unavailable and never calls GetTermForReport/GetReport for it; a client that pages to the end and caches the result silently persists an incomplete catalog. Nothing surfaces as an error, so there is no retry or fallback path.
Fix: the report catalog the ListReports handler enumerates — the catalog seed/materialisation in the artifact crate's state plus the ListReports handler that pages over it (services/service-com-amazonaws-artifact-artifact/src/ops/, src/state.rs). The paging envelope itself (nextToken emission) is not the defect here.
Traces: artifact-catalog-list-reports#task#1 (representative), artifact-list-pagination#task#2 (second page of the same enumeration, same cluster).

Cause 2 of 2 — ListComplianceInquiryQueries returns an empty queries list for an inquiry whose queries exist (fixture artifact-compliance-inquiry-basic-workflow, step task#3).
AWS: ListComplianceInquiryQueries (Smithy: restJson1 GET /v1/compliance-inquiry/listQueries, http code 200; input complianceInquiryId @required @httpQuery @pattern ^compliance-inquiry-[a-zA-Z0-9]{16}$, maxResults @range 1..300, nextToken @length 1..2048; output queries: QueriesList, nextToken; @paginated items=queries; @readonly; errors AccessDeniedException 403, ValidationException 400, ResourceNotFoundException 404, ThrottlingException 429, InternalServerException 500). This operation enumerates the queries recorded against one compliance inquiry, as QuerySummary entries (Smithy QuerySummary: queryIdentifier @required 'Sequential identifier of the query within the inquiry', query @required 'The actual query text', response 'Generated response to the query', citations: CitationList 'Supporting citations for the response', reviewType, status @required: QueryStatus, statusMessage @required, createdAt @required @timestampFormat date-time, updatedResponseVersions 'Ordered list of response version history entries, oldest first'). A query submitted to an inquiry earlier in the session must therefore appear in this enumeration, with its generated response and its citation list (each Citation carrying sourceContent, sourceLabel and sourceLink) attached.
Emulator: HTTP 200 with the queries collection empty — the replay reports '$.queries[0]: missing (expected {citations: [{sourceContent, sourceLabel, sourceLink}, ...], query, response, status, statusMessage, createdAt, queryIdentifier, ...}, got None)'. The query submitted against the inquiry earlier in the same fixture is not enumerated at all, so the inquiry's query history is not readable back.
Why: the inquiry workflow is submit-a-query then read the answer back; ListComplianceInquiryQueries is how a client retrieves the generated response, its status and the citations that support it. An empty 200 tells the client the inquiry has no queries, so it never renders or exports the answer it just asked for, and a client that polls this operation waiting for QueryStatus to leave PROCESSING polls forever against a list that never gains an element — no error is ever raised to break the loop. It also makes any downstream operation keyed on queryIdentifier (question-level feedback, a per-question export) unreachable, since the client has no identifier to name.
Fix: the compliance-inquiry query history in the artifact crate's state — queries submitted against an inquiry must be persisted with their generated response, citations, status/statusMessage and createdAt, and the ListComplianceInquiryQueries handler must page over them (services/service-com-amazonaws-artifact-artifact/src/ops/, src/state.rs).
Traces: artifact-compliance-inquiry-basic-workflow#task#3.

Note on neighbouring open issues: neither cause is covered by an open artifact issue. 20261029T112047Z-artifact-listreports-pagination concerns the ListReports pagination cursor's seal taking no server-side secret (a forged cursor being honoured) — that is the cursor's integrity, not Cause 1's missing catalog membership, and sealing the cursor correctly would still enumerate a short catalog. 20261029T192742Z-artifact-refusal-plane-answers-200-or-401-where covers four refusal-plane causes (PutComplianceInquiryFeedback, ExportComplianceInquiry, GetReport, ListTagsForResource) where an expected 400 is answered 200/401; both causes here are 200->200 payload-membership divergences on @readonly list operations and are outside it. 20261029T134423Z-artifact-getcomplianceinquirymetadata-maintainability is about hoisting the inquiry-resolution rule out of four handler copies, not about whether query history is persisted.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline (emulator_up true, score 76.7%, 7 of 30 steps mismatched, 6 divergence clusters; the other 4 clusters are already owned by 20261029T192742Z-artifact-refusal-plane-answers-200-or-401-where).

Cause 1 — fixture artifact-catalog-list-reports step task#1, ListReports (divergence_key service=artifact|ListReports|status=200->200; affected_count 2, also artifact-list-pagination#task#2):
  status: expected 200, got 200 (no status divergence)
  $.reports[arn="arn:aws:artifact:<region>::report/<reportId>:<version>"]: missing (expected the full ReportSummary for that report, got None) — reported for multiple distinct report arns, e.g. an ACR/VPAT accessibility-conformance report (series ACR, category 'Certifications and Attestations', state PUBLISHED, visibility PUBLIC, acceptanceType PASSTHROUGH, version 1) and a UAE TDRA IAR information-assurance assessment report (series 'UAE TDRA IAR', version 7, periodStart/periodEnd present). The arns, ids, versions and period timestamps above are the recording's values filling the ReportSummary template; the requirement is that every report the catalog holds is enumerated with its full summary, not that these particular ids exist.
  No unexpected/extra $.reports entries were reported, and no nextToken divergence.

Cause 2 — fixture artifact-compliance-inquiry-basic-workflow step task#3, ListComplianceInquiryQueries (divergence_key service=artifact|ListComplianceInquiryQueries|status=200->200|keys=$.queries[0]):
  status: expected 200, got 200 (no status divergence)
  $.queries[0]: missing (expected a QuerySummary carrying query, response, status, statusMessage, createdAt, queryIdentifier and a non-empty citations list of {sourceContent, sourceLabel, sourceLink} entries, got None) — the queries collection is empty, so the query submitted earlier in the same fixture is not enumerated.

Smithy (services/service-com-amazonaws-artifact-artifact/smithy-model/com-amazonaws-artifact-artifact.json):
  ListReports GET /v1/report/list, 200; output {reports: ReportsList, nextToken}; @paginated items=reports, inputToken/outputToken=nextToken, pageSize=maxResults; @readonly; errors 403/400/404/402/429/500.
  ReportSummary {id ^report-[a-zA-Z0-9]{16}$, arn, name, series, category, companyName, productName, description, state: PublishedState, version @range min 1, periodStart/periodEnd date-time, acceptanceType, uploadState, statusMessage}.
  ListComplianceInquiryQueries GET /v1/compliance-inquiry/listQueries, 200; input complianceInquiryId @required ^compliance-inquiry-[a-zA-Z0-9]{16}$; output {queries: QueriesList, nextToken}; @paginated items=queries; @readonly; errors 403/400/404/429/500.
  QuerySummary {queryIdentifier @required, query @required, response, citations: CitationList, reviewType, status @required: QueryStatus, statusMessage @required, createdAt @required date-time, updatedResponseVersions}.

## Steps to Reproduce
1. Boot the emulator from mainline and replay trace set simparity-wire-aws-v1.
2. Cause 1: run fixture artifact-catalog-list-reports to step task#1 — GET /v1/report/list (optionally with maxResults). Expect 200 whose $.reports enumerates every report in the catalog, each as a full ReportSummary keyed by its arn; the emulator answers 200 with those report arns absent from $.reports. Re-run fixture artifact-list-pagination to step task#2 to see the same reports missing from a later page of the same enumeration, confirming it is catalog membership and not one page's slicing.
3. Cause 2: run fixture artifact-compliance-inquiry-basic-workflow through its inquiry-create and query-submit steps, then step task#3 — GET /v1/compliance-inquiry/listQueries?complianceInquiryId=<the inquiry just created>. Expect 200 with $.queries[0] being the submitted query's QuerySummary (query text, generated response, status, statusMessage, createdAt, queryIdentifier and its citations); the emulator answers 200 with an empty queries list.
