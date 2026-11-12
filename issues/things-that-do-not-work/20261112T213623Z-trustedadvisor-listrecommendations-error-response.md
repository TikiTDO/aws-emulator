Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 03c5ea5bf7d0d25872bb092ddda325e89ed8563c

# Malformed nextToken is refused AFTER the support-level gate, so one malformed request is 403 on an unentitled plan and 400 on an entitled one

_Filed: 2026-11-12T21:36:23Z_
_Operation: ListRecommendations | Class: error-response_

> _Note: this issue was identified against the code base at commit `03c5ea5bf7d0d25872bb092ddda325e89ed8563c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states ONE ordering rule for the input-validation refusals it owns itself: a condition the model's constraint traits cannot express "is also ORDERED where the modelled ones are: ahead of the support-level gate, so a malformed request is this 400 whether the calling account's support plan includes the API or not" (src/service/validation.rs:53-55).

The crate has exactly two such handler-owned conditions. `BatchUpdateRecommendationResourceExclusion` obeys the rule: its repeated-ARN refusal is raised at src/ops/batch_update_recommendation_resource_exclusion.rs:285-293, BEFORE `crate::state::support_level_refusal` at :298. The other one — a `nextToken` that is not a cursor this read issued, which no constraint trait can express (the modelled `@length(4, 10000)` is satisfied) — breaks it in all SEVEN paginated handlers: each calls `support_level_refusal` as its first statement and only decodes the token inside `crate::state::paginate` much later.

Consequence: GET /v1/recommendations?nextToken=!!!! is answered 400 `ValidationException` for an account on Business/Enterprise-On-Ramp/Enterprise and 403 `AccessDeniedException` ("Access denied due to support level") for the same request from an account on Basic/Developer. The well-formedness of a request is a property of the request, not of the plan the caller is on — which is the crate's own sentence, and is exactly the rule a FIXED issue already established for the batch handler (`20261102T132244Z-trustedadvisor-batchupdaterecommendationresourceexclusion-error-response.md`). The seven handlers were never moved with it.

The fix is reachable without touching state: the cursor scope of every one of the seven reads is built from the REQUEST alone (`RecommendationFilters::scope`, `ResourceFilters::scope`, `crate::state::list_scope` over the request's filters), so `crate::state::decode_token(scope, token)` can be evaluated ahead of the gate exactly as the batch handler's own condition is.

Affected handlers (identical ordering): ListRecommendations, ListChecks, ListRecommendationResources, ListRecommendationsForResource, ListOrganizationRecommendations, ListOrganizationRecommendationAccounts, ListOrganizationRecommendationResources. Filed against `ListRecommendations` because the (ListChecks, error-response) pair is already held by the open issue `20261031T193638Z-trustedadvisor-listchecks-error-response.md`, which is about the WORDING of this same refusal (its ad-hoc sentence) and says nothing about where it is evaluated.

## Evidence

The rule, in the module that owns this service's input-validation refusals — services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/service/validation.rs:47-55:
```
//! A condition the model's constraint traits CANNOT express is refused by the handler that
//! owns it, so the framework's walk never collects it and it reaches no [`Violation`]. Such
//! a refusal is still one of this service's input-validation refusals and is worded here
//! too: ...
//! moves here — only the sentence. Such a refusal is also ORDERED where the modelled ones
//! are: ahead of the support-level gate, so a malformed request is this 400 whether the
//! calling account's support plan includes the API or not.
```

The sibling handler that obeys it — src/ops/batch_update_recommendation_resource_exclusion.rs:276-293 (its own comment states the ordering) …
```
        // Request well-formedness, ahead of the support-level gate and of every
        // lookup and every write: ... This is one of this service's input-validation
        // refusals (`crate::service::validation`), and those are answered where
        // the modelled constraints the centralized request walk collects are
        // answered: before the caller's entitlement is consulted, so one
        // malformed request has one status on every support plan.
        let repeated = repeated_arns(&input.recommendation_resource_exclusions);
        if let Some(message) = repeated_arn_refusal(&repeated) {
```
… and only then the gate, at src/ops/batch_update_recommendation_resource_exclusion.rs:298:
```
        if let Some(denied) = crate::state::support_level_refusal(&*self.engine) {
```

The seven list handlers do the opposite. src/ops/list_recommendations.rs:240 — the gate is the FIRST thing the handler does:
```
        if let Some(denied) = crate::state::support_level_refusal(&*self.engine) {
            return Err(ListRecommendationsErrorModeled::AccessDeniedException(denied).into());
```
and the token is only read at src/ops/list_recommendations.rs:294-305, 54 lines later:
```
        let page = crate::state::paginate(
            &filters.scope("recommendations"),
            matching,
            input.next_token.as_deref(),
            input.max_results,
        )
        .map_err(|_| {
            ListRecommendationsErrorModeled::ValidationException(ValidationException {
                message: "nextToken is not a pagination token this service issued for this list"
```
Same order in the six siblings: gate / paginate at list_checks.rs:194 / :230-248, list_recommendation_resources.rs:203 / :255-269, list_recommendations_for_resource.rs:244 / :312-330, list_organization_recommendations.rs:224 / :275-286, list_organization_recommendation_accounts.rs:208 / :270-287, list_organization_recommendation_resources.rs:191 / :276-293.

That the token refusal IS the handler's own (and not a modelled constraint the framework refuses ahead of everything) is stated by the crate's own suite — tests/all/recommendation_loop.rs:377-379: "each with a token that is not base64 of a key (the modelled `length 4..10000` is satisfied, so the refusal is the handler's own)".

Test coverage of the ordering rule as it stands: tests/all/support_level_gate.rs:309-378 (`shape_validation_precedes_the_gate`) pins ONLY the MODELLED constraints ahead of the gate — its seven rows are a short label, a bad pattern, maxResults below/above range, a bad enum, an empty list and a missing `@required` member; no row sends a malformed `nextToken`. tests/all/malformed_batch_precedes_the_gate.rs pins the handler-owned half of the rule for the batch handler only (its header at :7-14: "an input-validation refusal of this service is answered 400 `ValidationException` independently of the calling account's entitlement — it is evaluated ahead of the support-level gate — whether the condition is one the model's constraint traits express or one a handler owns because they cannot"). The nextToken suites (tests/all/recommendation_loop.rs:373-469, tests/all/cursor_scope_encoding.rs, tests/all/organization_recommendation_accounts.rs:443-530, tests/all/organization_recommendation_resources.rs:634-721, tests/all/organization_recommendations.rs:588-656) all drive an ENTITLED account only, so no test pins the 403 this reports.

## Steps to Reproduce

1. Read src/service/validation.rs:53-55: a handler-owned input-validation refusal of this service "is also ORDERED where the modelled ones are: ahead of the support-level gate, so a malformed request is this 400 whether the calling account's support plan includes the API or not".
2. Start the emulator with EMULATOR_TRUSTEDADVISOR_SUPPORT_PLAN=business (an entitled plan) and GET /v1/recommendations?nextToken=%21%21%21%21 (four characters, so the modelled @length(4,10000) is satisfied and the refusal is the handler's own). Observe 400 ValidationException.
3. Restart with EMULATOR_TRUSTEDADVISOR_SUPPORT_PLAN=basic (or developer) and send the SAME request. Observe 403 AccessDeniedException with message "Access denied due to support level" — the same malformed request now has a different status because of an account property that has nothing to do with the request.
4. Contrast with the batch handler's own handler-owned condition, which the crate already ordered correctly: on the SAME basic plan, PUT /v1/batch-update-recommendation-resource-exclusion naming one recommendation-resource ARN twice answers 400 ValidationException (src/ops/batch_update_recommendation_resource_exclusion.rs:285-293 runs before the gate at :298, and tests/all/malformed_batch_precedes_the_gate.rs pins it on every plan).
5. Repeat step 3 against /v1/checks, /v1/recommendations/{arn}/resources, /v1/recommendations-for-resource/{arn}, /v1/organization-recommendations, /v1/organization-recommendations/{arn}/accounts and /v1/organization-recommendations/{arn}/resources: six more handlers with the gate ahead of the token decode (gate / paginate line pairs listed in the evidence).
