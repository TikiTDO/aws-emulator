Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: 96b1cd775c53225a6d4bf8c1fc7ed76785b74794

# The page-token check only tests that the token names a stored report, not that this operation issued it — contradicting the rule the handler's doc, its refusal sentence and its test name all state

_Filed: 2026-11-04T19:08:14Z_
_Operation: DescribeReportDefinitions | Class: pagination_

> _Note: this issue was identified against the code base at commit `96b1cd775c53225a6d4bf8c1fc7ed76785b74794`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DescribeReportDefinitions` states, in three places in this crate, that a `NextToken` the operation did not issue is refused: the helper doc comment ("Refused with a 400 `ValidationException` when the token is not one this operation issued"), the refusal sentence it hands the caller ("Pass the NextToken a previous DescribeReportDefinitions response returned.") and the name of the test that pins it (`describe_refuses_a_next_token_it_did_not_issue`).

The implemented check is weaker: `resolve_page_token` hex-decodes the token and accepts it whenever the decoded bytes are the name of a report the account CURRENTLY HOLDS. Nothing records or verifies issuance. A token is only ever emitted for `store.reports.get(end)` — i.e. for a report at a page boundary (index >= the page size of 5) — so `hex::encode(<the name of the report at index 0>)` is a token this operation can never have issued, yet it is accepted and served as a page starting at index 0. Any caller that has seen a report name from a previous page (every caller, since `ReportName` is a member of every element of `ReportDefinitions`) can mint such a token, and the token scheme as written cannot distinguish a minted one from an issued one, so the refusal condition the crate advertises is not implemented at all.

No wrong data is served (the page that comes back is a coherent window starting at the named report), which is why this is LOW: the defect is the advertised refusal that never happens, and the doc/code divergence it leaves for the next reader of this handler.

## Evidence

The rule, stated in the crate:

- `src/ops/describe_report_definitions.rs:208-215`:
```
/// The index in `store` the token resumes at.
///
/// Refused with a 400 `ValidationException` when the token is not one this operation
/// issued, or names a report the account no longer holds: this operation models no
/// client-input error of its own, ...
```
- `src/ops/describe_report_definitions.rs:220-225` (the sentence the caller gets):
```
    let invalid = || {
        crate::scaffolded::Scaffolded::validation(
            "The NextToken in the request is not valid. Pass the NextToken a previous \
             DescribeReportDefinitions response returned.",
        )
    };
```
- `tests/all/report_definition_lifecycle.rs:929` names the rule: `async fn describe_refuses_a_next_token_it_did_not_issue()` — and its rows only cover tokens that fail to name a stored report (`"zzz"`, `"6669727"`, `hex::encode("ghost-report")`, lines 935-942), so a token that names a stored report the operation never offered is untested.

The code under those statements, `src/ops/describe_report_definitions.rs:226-230`:
```
    let report_name = hex::decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or_else(invalid)?;
    store.position(&report_name).ok_or_else(invalid)
```
The only predicate is `store.position(&report_name).is_some()` — "names a report the account currently holds". Issuance is never checked, and cannot be: the token is a pure function of the report name (`src/ops/describe_report_definitions.rs:204-206`, `fn encode_page_token(report_name: &str) -> String { hex::encode(report_name.as_bytes()) }`).

What the operation actually issues, `src/ops/describe_report_definitions.rs:180-183`:
```
        let next_token = store
            .reports
            .get(end)
            .map(|next| encode_page_token(&next.definition.report_name));
```
`end >= page_size` (5, `DEFAULT_PAGE_SIZE` at line 196, and `MaxResults` is pinned to 5 by the model), so no token for a report at index < 5 is ever issued, while every such token is accepted.

## Steps to Reproduce

1. Seed or hold several report definitions, e.g. names `r0`..`r10` in creation order (the lifecycle suite's `Cur::seed` does this at tests/all/report_definition_lifecycle.rs:131-133).
2. Call `DescribeReportDefinitions` with no paging members: it answers reports `r0`..`r4` plus `NextToken = hex("r5")` — the only token this call issued.
3. Call `DescribeReportDefinitions` with `NextToken = hex("r0")` (i.e. `"7230"`), a token the operation never issued and never could issue, which still satisfies the modelled pattern `^[A-Za-z0-9_\.\-=]*$`.
4. Observe HTTP 200 with the page starting at `r0`, instead of the 400 `ValidationException` ("Pass the NextToken a previous DescribeReportDefinitions response returned.") that the helper doc at src/ops/describe_report_definitions.rs:211-212 says is answered for a token the operation did not issue.
