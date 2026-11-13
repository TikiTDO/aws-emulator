Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 65fd8797851bb5c5937d1ea0792f5c718c73df47

# History NextToken is a positional offset into a newest-first, filtered listing, so a page boundary re-reports or skips a request

_Filed: 2026-10-27T22:21:24Z_
_Operation: ListRequestedServiceQuotaChangeHistory | Class: pagination_

> _Note: this issue was identified against the code base at commit `65fd8797851bb5c5937d1ea0792f5c718c73df47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`src/state.rs` justifies the request store's design with the claim that the append-only, insertion-ordered index is what makes a history pagination token stable. The code contradicts it: the history listings walk the insertion index in REVERSE (newest first) and then cut the page with `paginate`, whose token is nothing but a base64 offset into the list that was just built. Any change at the FRONT of that list — a new request appended, or a request leaving the filtered set — shifts every offset, so the token does not resume at the position it was issued for.

Two concrete, in-emulator observable consequences:
* a request minted between page 1 and page 2 pushes the listing down by one, so the last record of page 1 is reported AGAIN as the first record of page 2;
* with `Status` supplied (e.g. `CASE_OPENED`), the filtered listing SHRINKS as the clock settles requests (SUPPORT_CASE_OPEN_DELAY/REQUEST_DECISION_DELAY are 500 ms, and every one of these handlers calls `settle_due_requests` first), so offset N now points past the record that used to sit there and that record is never reported on any page.

The same positional-token scheme is used by `ListRequestedServiceQuotaChangeHistoryByQuota`.

## Evidence

The claim, `src/state.rs:18-22`:
```
//! * the **quota increase requests** (`RequestServiceQuotaIncrease`,
//!   `GetRequestedServiceQuotaChange`, `CreateSupportCase`,
//!   `ListRequestedServiceQuotaChangeHistoryByQuota`) — append-only, with an
//!   insertion-ordered index so a history listing is newest-first and a pagination
//!   token keeps meaning the same position;
```

The code — the index is walked reversed, `src/state.rs:1259-1267`:
```
        state
            .request_order
            .iter()
            .rev()
            .filter_map(|id| state.quota_requests.get(id))
            .filter(|request| service_code.is_none_or(|code| request.service_code == code))
            .filter(|request| cutoff.is_none_or(|cutoff| request.created >= cutoff))
```
(and identically `requests_for_quota`, `src/state.rs:1232-1239`).

The token is a plain offset into that freshly built list, `src/state.rs:2032-2036` and `2041-2042`:
```
    let items: Vec<T> = all.into_iter().skip(start).take(size).collect();
    let stopped_at = start + items.len();
    ...
        next_token: (stopped_at < total).then(|| encode_page_token(listing, stopped_at)),
...
fn encode_page_token(listing: &str, offset: usize) -> String {
    base64::engine::general_purpose::STANDARD.encode(format!("{listing}:{offset}"))
}
```
The handler builds the filtered list and pages it, `src/ops/list_requested_service_quota_change_history.rs:201-225` (status filter, level filter, then `crate::state::paginate(Self::NAME, history, input.max_results, input.next_token.as_deref())`), after `quotas_state.settle_due_requests(now)` at line 181 — which is what makes a status-filtered listing shrink between two pages.

Test coverage: `tests/all/requested_change_history.rs` walks pages over a listing that is never mutated mid-walk (`the_account_wide_history_lists_every_stored_request_newest_first`, line 109; `MaxResults=1` resume at lines 499-509), so nothing pins the resumed position across an append or a settlement.

## Steps to Reproduce

1. Create three quota increase requests A, B, C (distinct adjustable quotas) at the same region endpoint.
2. Call ListRequestedServiceQuotaChangeHistory with `{"MaxResults":2}` -> `[C, B]` plus a NextToken that encodes offset 2.
3. Create a fourth request D, then resume with `{"MaxResults":2,"NextToken":"<token>"}` -> the listing is now `[D, C, B, A]`, so the page returns `[B, A]` and B is reported on both pages.
4. For the skip variant: create several requests, call the listing with `{"Status":"CASE_OPENED","MaxResults":1}` and keep the token; wait ~600 ms so REQUEST_DECISION_DELAY settles the head of that filtered listing to APPROVED; resume with the token -> the record that occupied the next offset is never returned on any page.
