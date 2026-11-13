Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.update-usage-invalid-range-bad-request apigateway.usage-plan-quota-period-reset-window apigateway.usage-plan-quota-unknown-key-rejected
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.update-usage-invalid-range-bad-request|run#20|AssertionError|assert (err.get("Code"), err.get("Message")) == ('BadRequestException', 'Usage date range cannot exceed 90 days'), (
Issue-Key: apigateway.usage-plan-quota-period-reset-window|run#31|AssertionError|assert (used_after, remaining_after) == (0, 200), (
Issue-Key: apigateway.usage-plan-quota-unknown-key-rejected|run#16|AssertionError|assert (err.get("Code"), err.get("Message")) == expected, (
Issue-Commit: 03bbe43a8a5ebde1fc83c18c0454ae56c6488a2e

# apigateway: 3 net-new causes in usage-plan metering — GetUsage's 90-day range refusal and unknown-key refusal are worded wrong, and an UpdateUsagePlan quota repoint silently discards a per-key remaining already recorded by UpdateUsage

> _Note: this issue was identified against the code base at commit `03bbe43a8a5ebde1fc83c18c0454ae56c6488a2e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetUsage

## Description
Three distinct root causes in the API Gateway usage-plan metering surface (GetUsage / UpdateUsage / UpdateUsagePlan). All three are net-new: no open issue carries any of these three replay keys, and the nearest open neighbours (20261109T062731Z-apigateway-2-net-new-causes-a-cloudformation-stack.md, which covers a CFN UsagePlan/ApiKey/UsagePlanKey stack rolling back, and 20261027T004324Z-apigateway-api-gateway-per-method-usage-plan-throttle-overrides.md, which covers per-method THROTTLE overrides) are about different operations and do not cover the quota/usage-report behaviour below.

--- Cause 1 of 3: GetUsage's refusal of an over-wide date range uses an emulator-invented sentence instead of AWS's. ---
AWS: Smithy model, GetUsage (GET /usageplans/{usagePlanId}/usage, startDate/endDate both @required httpQuery members) declares BadRequestException (HTTP 400) — "The submitted request is not valid... See the accompanying error message for details." API Gateway's usage data is available over at most a 90-day window, and a GetUsage whose startDate..endDate span exceeds it is refused 400 BadRequestException with the fixed sentence `Usage date range cannot exceed 90 days`. That sentence is entirely fixed text: it interpolates nothing from the request (no dates, no plan id, no key id, no computed day count), so it is the same sentence for every over-wide range.
Emulator: the range is refused with the right code and status — 400 `BadRequestException` — but with a different sentence: `Invalid usage interval specified. The usage interval must be at most 90 days`. That string is built by `usage_interval_too_wide()` and interpolated from the crate's own `MAX_USAGE_INTERVAL_DAYS` constant, and it is documented in the crate as an "EMULATOR RESOURCE GUARD rather than a published quota" — i.e. the wording was chosen for the guard, not taken from AWS.
Why: a client that branches on the refusal sentence (operators and SDK wrappers routinely match the message for GetUsage, because BadRequestException is the single code for every malformed GetUsage input — bad startDate, bad endDate, reversed interval, over-wide interval) cannot tell an over-wide range from any other bad request, and a test or runbook asserting AWS's sentence fails against the emulator. The status and code alone do not identify the condition here.
Fix: the message constant/formatter for the over-wide interval behind the GetUsage date-range validation — `usage_interval_too_wide()` / `usage_interval()` in the apigateway crate's usage state module, which `GetUsage`'s handler maps into BadRequestException. The sentence must be AWS's fixed text, not a sentence derived from the emulator's own guard constant. (GetUsage's siblings for a non-date value and a reversed interval ride the same validator and should be checked against AWS's wording at the same time.)
Traces: apigateway.update-usage-invalid-range-bad-request — the check's positive control (UpdateUsage then an in-bounds 1-day GetUsage) succeeds, so the divergence is in the range refusal itself and not in GetUsage being unreachable for the plan.

--- Cause 2 of 3: an UpdateUsagePlan quota repoint silently discards the per-key remaining quota UpdateUsage already recorded for today. ---
AWS: Smithy model, UpdateUsage (PATCH /usageplans/{usagePlanId}/keys/{keyId}/usage) grants "a temporary extension to the remaining quota" of one (usage plan, API key) pair and returns the Usage report; GetUsage reports `items` (wire name `values`) as "daily logs of used and remaining quotas... indexed over the API keys in a usage plan", each entry "of the format [used quota, remaining quota]"; UpdateUsagePlan (PATCH /usageplans/{usagePlanId}) replaces the plan's QuotaSettings. Recorded behavior: after `UpdateUsage replace /remaining = <R>`, today's entry for that key reads `[0, <R>]` (UpdateUsage moves remaining, never used); a subsequent UpdateUsagePlan that repoints `/quota/period` (DAY->WEEK) and `/quota/limit` (to a larger value) is reflected immediately in GetUsagePlan, and today's ALREADY-RECORDED usage entry is left untouched — GetUsage for the same single day still reports `[0, <R>]`. The recorded remaining of a day is a fact about that day; repointing the plan's quota does not rewrite it.
Emulator: the recorded remaining is destroyed by the plan edit. GetUsage for the same day, immediately after the UpdateUsagePlan, reports `[0, <new plan limit>]` instead of `[0, <R>]` (the check records `[0, 200]` before the edit and reads `[0, 5000]` after it, 5000 being the plan's new `/quota/limit`). Root cause is how the grant is stored: `grant_remaining_quota` does not store the granted remaining, it stores a DELTA against the plan's budget at grant time (`remaining + spent - window.budget`) keyed by the quota PERIOD START computed from the plan's quota as it stood then. Repointing `/quota/period` moves that period start (a DAY window keyed on today becomes a WEEK window keyed on the week's first day), so `extension_of(window.start)` finds nothing and the grant is dropped; repointing `/quota/limit` additionally rescales every surviving delta, since the delta was computed against the old limit. Either patch alone corrupts the recorded entry.
Why: remaining quota is the number the next request is admitted against, so a client reading GetUsage after any unrelated plan edit is told a budget the key does not have — here an extension granted downward (200 of 1000) silently becomes 5000, i.e. the key is handed the whole new limit. Dashboards and quota-governance tooling that grant a temporary extension and then adjust the plan see the grant evaporate, and enforcement follows the wrong number because the same accounting feeds admission (`quota_admits`).
Fix: the per-key usage/quota state in the apigateway crate's usage state module — store what the grant STATES (the remaining quota for the pair on that day) as a fact keyed independently of the plan's current quota settings, so neither `UpdateUsagePlan /quota/period` nor `/quota/limit` can rewrite an already-recorded daily entry; `metered_quota_window` / `remaining_quota` / `daily_usage` must read that recorded value for days already recorded rather than recomputing it from the plan's present limit and present period boundary.
Traces: apigateway.usage-plan-quota-period-reset-window — the check verifies GetUsagePlan does report the new period/limit immediately (that part passes), so the defect is confined to the per-key usage entry.

--- Cause 3 of 3: GetUsage refuses an unknown keyId with the API-key reader's sentence instead of GetUsage's own. ---
AWS: Smithy model, GetUsage declares NotFoundException (HTTP 404) — "The requested resource is not found." — and `keyId` is "the Id of the API key associated with the resultant usage data". Recorded behavior: a GetUsage naming a keyId that corresponds to no API key is refused 404 `NotFoundException` with the fixed sentence `Invalid Key identifier specified`. Fixed text only; the rejected id is not quoted back. Note the sibling: UpdateUsage on the same unknown keyId answers `Invalid API Key content specified`, which the emulator already gets right — AWS words these two refusals DIFFERENTLY, and that is the point of the divergence.
Emulator: GetUsage answers 404 `NotFoundException` with `Invalid API Key identifier specified`. The handler reuses the shared `NO_SUCH_API_KEY` constant (the sentence GetApiKey/DeleteApiKey/tagging answer for an unknown `apiKey`), so GetUsage's keyId refusal is given the API-key reader's wording instead of its own.
Why: a client distinguishing "this plan does not meter that key" from "no such API key resource" by the refusal sentence — the only thing that differs, since both are 404 NotFoundException — is misled, and a test asserting AWS's GetUsage wording fails. It also makes GetUsage and UpdateUsage look like they share one refusal when AWS words them apart.
Fix: give the GetUsage handler's keyId refusal its own message constant (beside the existing `NO_SUCH_METERED_API_KEY` used by the grant path) instead of reusing the shared `NO_SUCH_API_KEY`; the constant belongs in the apigateway crate's usage state module with the surface's other usage sentences.
Traces: apigateway.usage-plan-quota-unknown-key-rejected — the check's earlier UpdateUsage assertion on the same unknown key PASSES, which pins the divergence to GetUsage's sentence alone.

## Evidence
Cause 1 — apigateway.update-usage-invalid-range-bad-request (mismatch):
  AssertionError: expected BadRequestException with the 90-day range message, got {'Message': 'Invalid usage interval specified. The usage interval must be at most 90 days', 'Code': 'BadRequestException'}
  Expected: ('BadRequestException', 'Usage date range cannot exceed 90 days') on GetUsage(usagePlanId=<plan>, keyId=<key>, startDate=today-400d, endDate=today).
  Actual:   ('BadRequestException', 'Invalid usage interval specified. The usage interval must be at most 90 days').
  Status/code agree (400 BadRequestException); the whole message is fixed text in both, so the entire sentence is the divergence.
  Emulator source of the wrong sentence: usage_interval_too_wide() in services/service-com-amazonaws-apigateway-backplanecontrolservice/src/state/usage.rs:420-425, reached from usage_interval() (same file, 432-444) via services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/get_usage.rs:201-206.

Cause 2 — apigateway.usage-plan-quota-period-reset-window (mismatch):
  AssertionError: expected repointing the quota period to leave today's already-recorded usage entry [0, 200] untouched, got [0, 5000]
  Sequence: CreateUsagePlan(quota={limit:1000, period:DAY}) -> CreateApiKey -> CreateUsagePlanKey -> GetUsage(today..today) indexes nothing for the key -> UpdateUsage(replace /remaining = 200) -> GetUsage(today..today) = [0, 200] (correct) -> UpdateUsagePlan(replace /quota/period = WEEK, replace /quota/limit = 5000) -> GetUsagePlan reports WEEK/5000 (correct) -> GetUsage(today..today) = [0, 5000] (WRONG, expected [0, 200]).
  The returned remaining equals the plan's NEW limit, i.e. the recorded grant was dropped, not merely rescaled.
  Emulator mechanism: grant_remaining_quota stores `remaining + spent - window.budget` keyed by quota_window(...).start (services/.../src/state/usage.rs:250-275); metered_quota_window looks the delta up by the CURRENT window start and adds it to the CURRENT limit (same file, 223-239); quota_period_start maps DAY->today but WEEK->the week's first day (204-213), so the DAY-keyed grant is unreachable after the period patch; remaining_quota/daily_usage then report budget-minus-spent = 5000 (335-378).

Cause 3 — apigateway.usage-plan-quota-unknown-key-rejected (mismatch):
  AssertionError: expected GetUsage to also reject the nonexistent key with ('NotFoundException', 'Invalid Key identifier specified'), got {'Message': 'Invalid API Key identifier specified', 'Code': 'NotFoundException'}
  Expected: ('NotFoundException', 'Invalid Key identifier specified') on GetUsage(usagePlanId=<plan>, keyId=<nonexistent>, startDate=today, endDate=today).
  Actual:   ('NotFoundException', 'Invalid API Key identifier specified').
  Same check's preceding assertion — UpdateUsage on the same nonexistent keyId must answer ('NotFoundException', 'Invalid API Key content specified') — PASSED, so UpdateUsage's wording is already right and only GetUsage's diverges.
  Emulator source of the wrong sentence: services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/get_usage.rs:191-196 returns crate::state::NO_SUCH_API_KEY, defined as "Invalid API Key identifier specified" in services/service-com-amazonaws-apigateway-backplanecontrolservice/src/state.rs:3649 (the sentence shared with the API-key readers/taggers at state.rs:3243 and 3309). The grant path's own correct sentence is NO_SUCH_METERED_API_KEY = "Invalid API Key content specified" at src/state/usage.rs:389.

## Steps to Reproduce
Re-replay the three behavioral checks listed in trace_ids. Each is self-contained against a booted emulator:

Cause 1 (apigateway.update-usage-invalid-range-bad-request):
  1. CreateUsagePlan(name=<unique>, quota={limit:1000, period:MONTH}, throttle={burstLimit:10, rateLimit:5.0}).
  2. CreateApiKey(name=<unique>, enabled=true); CreateUsagePlanKey(usagePlanId, keyId, keyType=API_KEY).
  3. UpdateUsage(usagePlanId, keyId, patchOperations=[{op:replace, path:/remaining, value:"10"}]) — succeeds and echoes usagePlanId.
  4. GetUsage(usagePlanId, keyId, startDate=today-1d, endDate=today) — positive control, succeeds.
  5. GetUsage(usagePlanId, keyId, startDate=today-400d, endDate=today).
     Expect 400 BadRequestException / 'Usage date range cannot exceed 90 days'; observe 400 BadRequestException / 'Invalid usage interval specified. The usage interval must be at most 90 days'.

Cause 2 (apigateway.usage-plan-quota-period-reset-window):
  1. CreateUsagePlan(quota={limit:1000, period:DAY}, throttle={rateLimit:10.0, burstLimit:20}); GetUsagePlan shows DAY/1000.
  2. CreateApiKey(enabled=true); CreateUsagePlanKey(..., keyType=API_KEY).
  3. GetUsage(usagePlanId, keyId, startDate=today, endDate=today) — the key is indexed in no entry yet.
  4. UpdateUsage(usagePlanId, keyId, patchOperations=[{op:replace, path:/remaining, value:"200"}]); GetUsage(today..today) now reports [0, 200].
  5. UpdateUsagePlan(usagePlanId, patchOperations=[{op:replace, path:/quota/period, value:"WEEK"}, {op:replace, path:/quota/limit, value:"5000"}]); GetUsagePlan reports WEEK/5000.
  6. GetUsage(usagePlanId, keyId, startDate=today, endDate=today).
     Expect the already-recorded entry unchanged at [0, 200]; observe [0, 5000] (the new plan limit).
     Patching only /quota/limit, or only /quota/period, is enough to corrupt the entry — rerun step 5 with each patch alone to see both halves of the cause.

Cause 3 (apigateway.usage-plan-quota-unknown-key-rejected):
  1. CreateUsagePlan(quota={limit:5, period:DAY}); CreateApiKey(enabled=true); CreateUsagePlanKey(..., keyType=API_KEY).
  2. UpdateUsage on the real key: patchOperations=[{op:replace, path:/remaining, value:"3"}].
  3. UpdateUsage(usagePlanId, keyId=<id that names no API key>, patchOperations=[{op:replace, path:/remaining, value:"1"}]) — must be 404 NotFoundException / 'Invalid API Key content specified' (already correct).
  4. GetUsage(usagePlanId, keyId=<same nonexistent id>, startDate=today, endDate=today).
     Expect 404 NotFoundException / 'Invalid Key identifier specified'; observe 404 NotFoundException / 'Invalid API Key identifier specified'.
  5. GetUsage on the real, associated key still reports remaining=3 (unaffected — this part already passes).
