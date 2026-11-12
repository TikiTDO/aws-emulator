Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: service-quotas.quota-utilization-report-pagination-consistency
Issue-Shape-Id: com.amazonaws.servicequotas#ServiceQuotasV20190624
Issue-Key: service-quotas.quota-utilization-report-pagination-consistency|run#7|AssertionError|assert not completed.get("NextToken"), (
Issue-Commit: 3c42ba1b0bd2ccb43c9358bd117a182df6732fd7

# GetQuotaUtilizationReport report snapshot spans the whole default-quota catalog, so an unpaginated read is truncated and returns a NextToken; paginated TotalCount also reports the grand total instead of the page's own count

> _Note: this issue was identified against the code base at commit `3c42ba1b0bd2ccb43c9358bd117a182df6732fd7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQuotaUtilizationReport

## Description
Two root causes in the Service Quotas quota-utilization-report path, both exercised by the single failing check service-quotas.quota-utilization-report-pagination-consistency.

Cause 1 of 2: a completed report read with NO MaxResults comes back truncated with a NextToken.
  AWS: Smithy (com.amazonaws.servicequotas#GetQuotaUtilizationReport) models input MaxResults as MaxResultsUtilization @range 1..1000 and documents "The default value is 1,000 and the maximum allowed value is 1,000"; output Quotas targets QuotaUtilizationInfoList @length 0..1000, and output NextToken is documented as "A token that indicates more results are available... If this field is not present, you have retrieved all available results." Recorded behavior only for the report's size: a real COMPLETED report read without MaxResults is a SINGLE page — it carries its whole record set and NO NextToken — which is why the check uses that unpaginated read as the ground-truth reference set. In other words the set of quotas AWS puts in one utilization report fits inside one 1,000-record page.
  Emulator: the report snapshot is built as one record per row of the entire seeded default-quota catalog (crate state: utilization_snapshot maps over self.default_quotas, one QuotaUtilizationInfo per catalog row), which is thousands of rows across all seeded service codes. GetQuotaUtilizationReport then pages that snapshot with the unasked page limit UTILIZATION_REPORT_PAGE_SIZE = 1_000, so the first page stops at 1,000 records and the response carries a NextToken. The check fails at its reference read with: AssertionError: unpaginated GetQuotaUtilizationReport unexpectedly returned a NextToken; the reference read must be a single page to be trustworthy.
  Why: a client that follows the documented contract — call GetQuotaUtilizationReport once, treat the absence of NextToken as "that was everything" — is handed a partial, utilization-sorted prefix of the account's quotas against the emulator where real AWS hands it the complete report. Any consumer that sizes its report handling on "one page is the whole report" (dashboards, drift checks, the reference-vs-paginated comparison this check performs) diverges, and callers that do page now walk thousands of records real AWS never emits.
  Fix: the report-snapshot construction in the service's state (the quota-utilization snapshot builder that currently enumerates the whole default-quota catalog) must select the record set a real utilization report contains, so a report fits the single ≤1,000-record page real AWS answers with and the unasked read emits no NextToken. The page limit itself (1,000) matches the model and is not the defect.
  Traces: service-quotas.quota-utilization-report-pagination-consistency (fails at the unpaginated reference read).

Cause 2 of 2: on a paginated read, TotalCount is the report's grand total rather than the page's own item count.
  AWS: the model documents TotalCount (range 0..2147483647) as "The total number of quotas included in the report across all pages", but recorded behavior only, and the recording contradicts the prose: real AWS's paginated TotalCount mirrors THAT page's own item count, including on a truncated final page. The check encodes exactly this observed contract (TotalCount == len(page["Quotas"]) for every page walked with MaxResults=5).
  Emulator: the GetQuotaUtilizationReport handler computes total_count from the full snapshot length before paging (report.quotas.len()) and returns that same grand total on every page, so a MaxResults=5 page carrying 5 records reports TotalCount equal to the whole report's record count. This cause is reached only once Cause 1 no longer aborts the check at its reference read, and is visible in the handler's response assembly.
  Why: a client using TotalCount to size a page's payload, to verify it received the page it asked for, or to drive a progress indicator per page gets a number that has nothing to do with the page in hand; a client comparing TotalCount against len(Quotas) as a consistency check sees every page as inconsistent.
  Fix: the TotalCount member population in the GetQuotaUtilizationReport handler's response serializer — set it from the page actually being returned, not from the pre-page snapshot length.
  Traces: service-quotas.quota-utilization-report-pagination-consistency (its per-page TotalCount assertion).

No open issue covers this operation: the existing service-quotas pagination issue 20261027T222124Z-service-quotas-listrequestedservicequotachangehistory-pagination.md is about ListRequestedServiceQuotaChangeHistory token semantics, a different listing and a different cause.

## Evidence
Failing check: service-quotas.quota-utilization-report-pagination-consistency [mismatch]

Cause 1 — expected vs actual:
  Expected (real AWS): GetQuotaUtilizationReport(ReportId=<id>) with no MaxResults on a COMPLETED report -> HTTP 200, Status=COMPLETED, Quotas = the report's whole record set, NO NextToken member present.
  Actual (emulator): HTTP 200, Status=COMPLETED, Quotas truncated to the 1,000-record default page, NextToken present.
  Replay output:
    File "check_service_quotas_quota_utilization_report_pagination_consistency.py", line 45, in run
      assert not completed.get("NextToken"), (
    AssertionError: unpaginated GetQuotaUtilizationReport unexpectedly returned a NextToken; the reference read must be a single page to be trustworthy
  Source of the oversized snapshot: services/service-com-amazonaws-servicequotas-servicequotasv20190624/src/state.rs, utilization_snapshot() (~line 1734) maps over self.default_quotas — one record per catalog row, thousands of rows; the unasked page limit is UTILIZATION_REPORT_PAGE_SIZE = 1_000 (state.rs ~line 427), applied at services/service-com-amazonaws-servicequotas-servicequotasv20190624/src/ops/get_quota_utilization_report.rs lines 231-238.

Cause 2 — expected vs actual:
  Expected (real AWS, observed): for every page of GetQuotaUtilizationReport(ReportId=<id>, MaxResults=5), TotalCount == number of records in that page (5 on a full page, fewer on the truncated last page).
  Actual (emulator): TotalCount is the grand total of the report's snapshot on every page, so a 5-record page reports the whole report's count.
  Source: services/service-com-amazonaws-servicequotas-servicequotasv20190624/src/ops/get_quota_utilization_report.rs line 223 (let total_count = report.quotas.len();) and line 263 (total_count: Some(i32::try_from(total_count)...)), both taken before/independently of the page cut made at lines 231-247.

No status code or error code diverges here: both calls return HTTP 200. The divergence is in the response representation (presence of NextToken, value of TotalCount), so no error message text is at issue.

## Steps to Reproduce
1. Boot the emulator and point a Service Quotas client at it.
2. Call StartQuotaUtilizationReport() and keep the returned ReportId.
3. Poll GetQuotaUtilizationReport(ReportId=<id>) until Status == "COMPLETED".
4. Cause 1: inspect that completed response — the emulator includes a NextToken and only 1,000 Quotas records; real AWS returns the report's whole record set in this single page with no NextToken.
5. Cause 2: call GetQuotaUtilizationReport(ReportId=<id>, MaxResults=5) and walk the NextToken chain. Each page carries at most 5 records but reports TotalCount equal to the report's grand total; real AWS reports TotalCount equal to that page's own record count.
6. Equivalently: replay the behavioral check service-quotas.quota-utilization-report-pagination-consistency.
