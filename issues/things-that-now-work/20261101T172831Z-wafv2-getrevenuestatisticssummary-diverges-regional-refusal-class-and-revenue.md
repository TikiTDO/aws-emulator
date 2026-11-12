Issue-Source: trace-sweep-divergence
Issue-Trace: wafv2-revenue-statistics-dashboard
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wafv2-revenue-statistics-dashboard wafv2-revenue-statistics-workflow
Issue-Shape-Id: com.amazonaws.wafv2#AWSWAF_20190729
Issue-Key: wafv2|GetRevenueStatisticsSummary|4xx->4xx|$.Message
Issue-Key: wafv2|GetRevenueStatisticsSummary|4xx->4xx|$.__type
Issue-Key: wafv2|GetRevenueStatisticsSummary|2xx->2xx|$.RevenueBreakdown.TotalAmount
Issue-Key: wafv2|GetRevenueStatisticsSummary|2xx->2xx|$.RevenueBreakdown.UnverifiedAmount
Issue-Key: wafv2|GetRevenueStatisticsSummary|2xx->2xx|$.RevenueBreakdown.VerifiedAmount
Issue-Commit: 4eb22cddbc76ccb41ded4251238efc1a94dbd0a6

# wafv2 GetRevenueStatisticsSummary diverges: regional refusal class and revenue amount formatting

> _Note: this issue was identified against the code base at commit `4eb22cddbc76ccb41ded4251238efc1a94dbd0a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRevenueStatisticsSummary

## Description
The AI-bot monetization summary operation diverges from WAF on both of its paths: the REGIONAL-scope refusal carries the wrong exception class and no Message, and the amounts it does serve are not formatted the way WAF formats a MonetizationAmountValue. This is ONE issue with TWO causes; both must be closed before the fix is submitted.

Cause 1 of 2: GetRevenueStatisticsSummary with Scope=REGIONAL refuses with the wrong exception class and omits the Message entirely.
AWS: Smithy: GetRevenueStatisticsSummary models WAFInvalidOperationException (client, httpError 400, single member `Message`) alongside WAFInvalidParameterException and WAFNonexistentItemException; Scope is a required `Scope` enum. Monetization is a CloudFront-only feature, so a REGIONAL-scope call is an invalid OPERATION for that scope, not a malformed parameter. WAF answers HTTP 400 with __type `WAFInvalidOperationException` and Message set to the fixed sentence `Monetization is not available for regional resources. Use CloudFront scope to configure monetization.` — that whole sentence is fixed text, with no request- or state-derived parts.
Emulator: HTTP 400, but __type `WAFInvalidParameterException`, and no `Message` member at all (nor the Field/Parameter/Reason members that class would carry).
Why: a client's error handling branches on the class — invalid-operation means "this feature is not available in this scope, switch to CLOUDFRONT", invalid-parameter means "fix the value you sent". The emulator pushes the caller down the wrong branch, and because the Message is absent the caller also gets no remediation text to log or surface, so a monetization dashboard reports a bad request instead of an unsupported scope. The open issue 20261029T192556Z-wafv2-refusals-carry-the-wrong-exception-class-sentence.md files this same monetization scope gate for ListSettlementRecords (its Cause 7) but does not cover GetRevenueStatisticsSummary, which is why this cause is net-new; the fix should share one scope gate between them.
Fix: the scope precondition in the GetRevenueStatisticsSummary handler's validation — gate on Scope before any parameter validation and raise the modeled WAFInvalidOperationException with the monetization sentence.
Traces: wafv2-revenue-statistics-dashboard#task#1.

Cause 2 of 2: the RevenueBreakdown amount members are serialized as a bare integer instead of WAF's fixed-scale decimal string.
AWS: Smithy: RevenueBreakdown.TotalAmount / VerifiedAmount / UnverifiedAmount all target `MonetizationAmountValue`, which is a STRING (not a numeric shape), so the service controls its textual form; the model states no pattern, so this is recorded behavior only. The recording shows WAF rendering every monetization amount as a decimal string with a fixed six-digit fraction — a zero balance is rendered `0.000000`, i.e. the integer part followed by `.` and exactly six fractional digits, for whatever the account's amount happens to be.
Emulator: serves the same three members as the string `0` — integer-formatted, with no decimal point and no fractional digits.
Why: the member is a string precisely so the client can hand it to a fixed-scale decimal/currency parser without binary float rounding. A consumer that parses on the documented shape (or compares/stores the string as the canonical money representation, or appends a currency suffix for display) sees a different scale from AWS, so sums and ledger comparisons built against real WAF do not round-trip against the emulator, and a strict six-decimal parser fails outright.
Fix: the response serializer for RevenueBreakdown in the GetRevenueStatisticsSummary handler (and any shared monetization-amount formatter it uses) — format every MonetizationAmountValue with the fixed six-place fraction rather than letting an integer or float render itself.
Traces: wafv2-revenue-statistics-workflow#task#3.

## Evidence
Replay of simparity-wire-aws-v1 (emulator_up=true, 198 steps, 154 passed). Both clusters are verdict=mismatch and neither is stub-blocked or owned by an open issue (stub_path empty, owned_by empty).

Cause 1 — GetRevenueStatisticsSummary, fixture wafv2-revenue-statistics-dashboard step task#1, expected_status 400 / actual_status 400:
  "$.__type: value_mismatch (expected 'WAFInvalidOperationException', got 'WAFInvalidParameterException')"
  "$.Message: missing (expected 'Monetization is not available for regional resources. Use CloudFront scope to configure monetization.', got None)"
The expected sentence is entirely fixed text for this error class; nothing in it comes from the request or from emulator state.

Cause 2 — GetRevenueStatisticsSummary, fixture wafv2-revenue-statistics-workflow step task#3, expected_status 200 / actual_status 200:
  "$.RevenueBreakdown.TotalAmount: value_mismatch (expected '0.000000', got '0')"
  "$.RevenueBreakdown.UnverifiedAmount: value_mismatch (expected '0.000000', got '0')"
  "$.RevenueBreakdown.VerifiedAmount: value_mismatch (expected '0.000000', got '0')"
The numeric value 0 here is this corpus's state (a fresh account with no monetization revenue); the requirement is the fixed six-place decimal FORM of whatever amount the account holds, not the literal string '0.000000'.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Cause 1: call GetRevenueStatisticsSummary with Scope=REGIONAL (plus a required TimeWindow and Currency) against a regional endpoint — fixture wafv2-revenue-statistics-dashboard, step task#1. Expect HTTP 400 with __type WAFInvalidOperationException and Message 'Monetization is not available for regional resources. Use CloudFront scope to configure monetization.'; the emulator returns WAFInvalidParameterException with no Message.
3. Cause 2: call GetRevenueStatisticsSummary with Scope=CLOUDFRONT on an account with no monetization revenue — fixture wafv2-revenue-statistics-workflow, step task#3. Expect HTTP 200 with RevenueBreakdown.TotalAmount/VerifiedAmount/UnverifiedAmount each a six-decimal string ('0.000000' for a zero balance); the emulator returns the integer-formatted string '0' for all three.
