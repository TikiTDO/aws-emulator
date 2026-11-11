Issue-Source: trace-sweep-divergence
Issue-Trace: pricing-plan-manager-create-subscription-approval-mode-and-resource-rules
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pricing-plan-manager-create-subscription-approval-mode-and-resource-rules pricing-plan-manager-create-subscription-validation-surface pricing-plan-manager-free-subscription-token-replay-and-etag-rules
Issue-Shape-Id: com.amazonaws.pricingplanmanager#AWSPricingPlanManager
Issue-Key: pricing-plan-manager|CreateSubscription|4xx->2xx|$.message
Issue-Key: pricing-plan-manager|CreateSubscription|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: pricing-plan-manager|CreateSubscription|4xx->2xx|status
Issue-Key: pricing-plan-manager|CreateSubscription|4xx->4xx|$.message
Issue-Key: pricing-plan-manager|CreateSubscription|4xx->2xx|$.resourceId
Issue-Commit: af64552852b9de70172a1fcfdec68b8a2bd1c743

# CreateSubscription admits invalid resource ARNs and clientToken parameter conflicts, and renders the length-constraint message without the offending value

> _Note: this issue was identified against the code base at commit `af64552852b9de70172a1fcfdec68b8a2bd1c743`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSubscription

## Description
Three net-new root causes in CreateSubscription's request-validation surface. Two are refusals the emulator does not issue at all (it answers 200 and mints a subscription), one is a refusal it issues with a message AWS does not use. None is marked ALREADY FILED or OWNED in the replay report.

Cause 1 of 3: CreateSubscription never validates the syntax of the ARNs in resourceArns, so a resource ARN that cannot name a real resource is accepted and a subscription is minted over it.
AWS: Smithy (services/service-com-amazonaws-pricingplanmanager-awspricingplanmanager/smithy-model/com-amazonaws-pricingplanmanager-awspricingplanmanager.json): CreateSubscription.resourceArns is @required ResourceArns (@length 1..10) and the operation models ValidationException (httpError 400, @required message + optional resourceId, doc "The request failed a business rule validation"). The syntax rule itself is recorded behavior only: an entry whose ARN is not well formed for the plan family's resource type is refused before anything is created, with HTTP 400 and Error.Message "Invalid resource ARN: <arn>" — fixed text "Invalid resource ARN: " plus the offending ARN copied verbatim from the request (in the recording a CloudFront ARN whose distribution id is not a CloudFront distribution id). The refusal creates nothing.
Emulator: the same CreateSubscription answers HTTP 200 and returns a fully formed ACTIVE (or PENDING_APPROVAL) subscription over the bogus ARN, with no message and no x-amzn-errortype header. This is the widest cluster in the corpus: 9 steps across 3 fixtures (the whole approval-mode-and-resource-rules fixture, two steps of the validation-surface fixture, and one step of the token-replay fixture) are answered 200 where AWS answered 400. src/ops/create_subscription.rs validates only the approvalMode enum and account capacity before minting; resourceArns is moved into the record without any per-entry ARN parse.
Why: a client's resource ARN typo, a half-rendered template variable or an id from the wrong account is silently accepted, so the account ends up holding a flat-rate subscription over a resource that does not exist. Every error path the client wrote for this refusal is dead, and because the emulator then reports that subscription from GetSubscription/ListSubscriptions, reconciliation against AWS shows a subscription AWS would never have created. It also hides the rest of the validation surface: six of the nine clustered steps are later assertions in those fixtures that were never reached behind this missing gate.
Fix: CreateSubscription's input validation in its handler (src/ops/create_subscription.rs, ahead of the mint/insert in crate::state) — parse every resourceArns entry as an ARN and check it against the resource types the plan family covers, returning the modeled ValidationException (400) with the message above naming the first offending ARN, mutating nothing. The same parse belongs in AssociateResourcesToSubscription, which takes the same member.
Traces: pricing-plan-manager-create-subscription-approval-mode-and-resource-rules#task#1 (cluster of 9 steps, spanning that fixture's task#1-#6, pricing-plan-manager-create-subscription-validation-surface#task#1-#2 and pricing-plan-manager-free-subscription-token-replay-and-etag-rules#task#9).

Cause 2 of 3: the resourceArns @length refusal renders the offending value as an item count ("11 items") instead of the list of values AWS prints.
AWS: Smithy: resourceArns @length(min 1, max 10). AWS's standard constraint-violation sentence quotes the rejected VALUE: "1 validation error detected: Value '[<arn-1>, <arn-2>, ..., <arn-11>]' at 'resourceArns' failed to satisfy constraint: Member must have length less than or equal to 10". The fixed text is "1 validation error detected: Value '", "' at '", "' failed to satisfy constraint: Member must have length less than or equal to "; the variable parts are the submitted list rendered as a bracketed, comma-and-space separated sequence of its entries, the member name resourceArns, and the modeled maximum 10. Status and class are already right here (400).
Emulator: it answers 400 with "1 validation error detected: Value '11 items' at 'resourceArns' failed to satisfy constraint: Member must have length less than or equal to 10" — the value slot carries a count, not the list. The length check itself is correct; only the rendering of the value is wrong.
Why: the message is the only machine- and human-readable statement of what was rejected. Tooling that parses AWS's constraint sentence to recover the offending value (and tests that assert on it) fails, and an operator reading the log cannot see WHICH ARNs were sent. This value rendering is shared by every @length constraint on a list in this service, so the defect is not specific to one call.
Fix: the constraint-violation message rendering used by CreateSubscription's validation (the shared validation-message helper this service's handlers call, invoked from src/ops/create_subscription.rs) — render a rejected list value as its bracketed, comma-separated entries rather than an "<n> items" summary. Note: the open issue 20261109T121305Z-pricing-plan-manager-2-net-new-root-causes.md lists this same oversized-resourceArns sentence as an assertion its check never reached; it carries no replay key for it, so this is where the wire divergence is filed.
Traces: pricing-plan-manager-create-subscription-validation-surface#task#3 (cluster of 2, with #task#4).

Cause 3 of 3: a clientToken replay carrying DIFFERENT parameters is answered as an idempotent hit (200) instead of being refused as a conflict.
AWS: Smithy: CreateSubscription.clientToken targets IdempotencyToken with @idempotencyToken and @length 1..64; the operation models ConflictException (httpError 409, @required message + @required resourceId, doc "The request conflicts with the current state of the resource"). Recorded behavior: a replay with the same token and identical parameters returns the original subscription, but a replay with the same token and different parameters is refused HTTP 409, Error.Code "ConflictException", Error.Message "The clientToken matches a previous request but the request parameters differ. Reuse a clientToken only for an identical request." — the whole sentence is fixed text, carrying nothing from the request — and resourceId naming the ARN of the subscription that token already minted. The refusal changes nothing.
Emulator: the differing replay is answered HTTP 200 with the originally stored subscription; message, resourceId and x-amzn-errortype are all absent. src/ops/create_subscription.rs looks the token up in state.idempotency (the token -> ARN map at src/state.rs:343) and returns the existing record unconditionally; SubscriptionRecord.client_token (src/state.rs:201) is stored but never compared, and nothing compares planFamily/planTier/usageLevel/resourceArns/approvalMode with the record the token points at.
Why: clientToken is the client's safe-retry mechanism. A client that retries after editing the request is told 200 and reads back a subscription that is not what it asked for — it believes it bought the new tier or the new resource set and is silently on the old one, with no error to surface. AWS makes that mistake loud. The open issue 20261111T004423Z-cloudfront-pricing-plan-manager-wafv2-4-net-new.md describes the same gate as its Cause 1 but is a behavioral finding with no wire replay key, so this fixture step is not covered by it.
Fix: the idempotency branch of CreateSubscription's handler (src/ops/create_subscription.rs) — compare the incoming request's identity-bearing members against the record the token resolves to, return the stored subscription only when they are identical, and otherwise return the modeled ConflictException (409) with the sentence above and resourceId set to the existing subscription's ARN, mutating nothing.
Traces: pricing-plan-manager-free-subscription-token-replay-and-etag-rules#task#8.

## Evidence
Replay of trace set simparity-wire-aws-v1 (42 mismatched steps of 78). ARNs, ids and markers below are per-run values filling the templates.

Cause 1 — fixture pricing-plan-manager-create-subscription-approval-mode-and-resource-rules step task#1 (CreateSubscription, expected 400, got 200; cluster of 9 steps):
  status: expected 400, got 200
  $.message: missing — expected "Invalid resource ARN: arn:aws:cloudfront::000000000000:distribution/sim2real-probe-ppm-dist-25ce7a5b" (fixed text "Invalid resource ARN: " + the offending ARN from the request), got None
  $headers.x-amzn-errortype: missing — expected the error type header, got None
  Also affected (same cause): that fixture's task#2-#6, pricing-plan-manager-create-subscription-validation-surface#task#1-#2, pricing-plan-manager-free-subscription-token-replay-and-etag-rules#task#9.
  Smithy: resourceArns @required @length(1,10); ValidationException httpError 400.

Cause 2 — fixture pricing-plan-manager-create-subscription-validation-surface step task#3 (CreateSubscription, 400 vs 400; cluster of 2 with task#4):
  $.message: expected "1 validation error detected: Value '[arn:aws:cloudfront::000000000000:distribution/sim2real-probe-ppm-toomany-b2d5858d-0, ... , arn:aws:cloudfront::000000000000:distribution/sim2real-probe-ppm-toomany-b2d5858d-10]' at 'resourceArns' failed to satisfy constraint: Member must have length less than or equal to 10"
  got "1 validation error detected: Value '11 items' at 'resourceArns' failed to satisfy constraint: Member must have length less than or equal to 10"
  (The 11 ARNs are the request's own values; the requirement is that the value slot holds the bracketed list, not a count.)

Cause 3 — fixture pricing-plan-manager-free-subscription-token-replay-and-etag-rules step task#8 (CreateSubscription, expected 409, got 200):
  status: expected 409, got 200
  $.message: missing — expected "The clientToken matches a previous request but the request parameters differ. Reuse a clientToken only for an identical request." (entire sentence fixed text), got None
  $.resourceId: missing — expected the ARN of the subscription the token already minted, got None
  $headers.x-amzn-errortype: missing — expected the ConflictException error type, got None
  Smithy: clientToken @idempotencyToken @length(1,64); ConflictException httpError 409 with required message + resourceId.
  Source: src/ops/create_subscription.rs (unconditional idempotency hit), src/state.rs:343 (token -> ARN map), :201 (client_token stored, never compared).

## Steps to Reproduce
Point a client at the emulator (region us-east-1, pricing-plan-manager endpoint).

Cause 1:
1. CreateSubscription(planFamily="CloudFront", planTier="FREE", resourceArns=["arn:aws:cloudfront::000000000000:distribution/not-a-distribution-id", <a valid web ACL ARN>]).
2. Observe HTTP 200 and a minted subscription. AWS refuses: HTTP 400, ValidationException, "Invalid resource ARN: arn:aws:cloudfront::000000000000:distribution/not-a-distribution-id", and creates nothing (ListSubscriptions must still not report it).

Cause 2:
1. CreateSubscription(planFamily="CloudFront", planTier="PRO", resourceArns=[<11 distinct ARNs>]).
2. Observe HTTP 400 with the value slot rendered "11 items". AWS renders the bracketed, comma-separated list of the 11 submitted ARNs in that slot, with the rest of the sentence identical.

Cause 3:
1. CreateSubscription(planFamily="CloudFront", planTier="FREE", resourceArns=[<dist ARN>, <acl ARN>], clientToken="t-1"); note the returned arn and planTier.
2. Repeat the identical call with clientToken="t-1" — must return the same subscription (this already works).
3. Call again with clientToken="t-1" but planTier="PRO" (or a different resourceArns set).
4. Observe HTTP 200 returning the stored FREE subscription. AWS refuses: HTTP 409 ConflictException "The clientToken matches a previous request but the request parameters differ. Reuse a clientToken only for an identical request." with resourceId = the existing subscription's ARN, and leaves state untouched.

Replay: replay_wire_corpus and re-run pricing-plan-manager-create-subscription-approval-mode-and-resource-rules#task#1, pricing-plan-manager-create-subscription-validation-surface#task#3 and pricing-plan-manager-free-subscription-token-replay-and-etag-rules#task#8.
