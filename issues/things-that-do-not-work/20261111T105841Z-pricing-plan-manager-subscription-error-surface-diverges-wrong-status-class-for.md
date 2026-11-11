Issue-Source: trace-sweep-divergence
Issue-Trace: pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle pricing-plan-manager-subscription-mutations-on-unknown-well-formed-arn pricing-plan-manager-subscription-arn-validated-before-mutation pricing-plan-manager-list-subscriptions-empty-and-invalid-token pricing-plan-manager-free-subscription-token-replay-and-etag-rules
Issue-Shape-Id: com.amazonaws.pricingplanmanager#AWSPricingPlanManager
Issue-Key: pricing-plan-manager|GetSubscription|4xx->4xx|$.message
Issue-Key: pricing-plan-manager|CancelSubscription|4xx->4xx|$.message
Issue-Key: pricing-plan-manager|GetSubscription|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: pricing-plan-manager|GetSubscription|4xx->4xx|status
Issue-Key: pricing-plan-manager|ListSubscriptions|4xx->4xx|$.message
Issue-Key: pricing-plan-manager|CancelSubscription|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: pricing-plan-manager|CancelSubscription|4xx->4xx|status
Issue-Commit: af64552852b9de70172a1fcfdec68b8a2bd1c743

# Subscription error surface diverges: wrong status class for malformed ARNs and stale ETags, and emulator-authored messages

> _Note: this issue was identified against the code base at commit `af64552852b9de70172a1fcfdec68b8a2bd1c743`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSubscription

## Description
Four net-new root causes in this service's refusal surface. Two change the HTTP status and error code a client branches on; two serve emulator-authored prose where AWS has fixed wording. None is marked ALREADY FILED or OWNED in the replay report.

Cause 1 of 4: the not-found refusal carries an emulator-authored message instead of AWS's sentence, on every operation that resolves a subscription ARN.
AWS: Smithy (services/service-com-amazonaws-pricingplanmanager-awspricingplanmanager/smithy-model/com-amazonaws-pricingplanmanager-awspricingplanmanager.json): ResourceNotFoundException is httpError 404 with @required message and @required resourceId, documented "The specified subscription was not found. Verify that the ARN is correct and that the subscription belongs to your account." Recorded behavior: Error.Message "Subscription not found: <arn>. Verify the ARN is correct and the subscription exists in your account." — fixed text "Subscription not found: " and ". Verify the ARN is correct and the subscription exists in your account.", with the arn copied verbatim from the request. The status (404) and code (ResourceNotFoundException) are already correct in the emulator; only the message diverges, and it diverges identically on the read path and the mutation path.
Emulator: "no subscription with ARN <arn>" — a different sentence that drops AWS's remediation clause. GetSubscription's miss arm in src/ops/get_subscription.rs formats that message, and the PreconditionFailure::NotFound arm of the mutators (src/state.rs:312-317 feeding src/ops/cancel_subscription.rs and the other ARN-taking handlers) does the same.
Why: the message is part of the error contract. Clients and operators match on it, log it and surface it to users; a test or runbook written against AWS's sentence does not match, and the emulator's wording omits the account/ARN guidance AWS gives.
Fix: the not-found message for the subscription resource — the shared refusal wording in crate::state (beside IF_MATCH_CONFLICT_MESSAGE at src/state.rs:325) used by every ARN-taking handler (get_subscription.rs, cancel_subscription.rs, cancel_subscription_change.rs, associate/disassociate, update_subscription, approve_paid_subscription), so all of them state it identically with the request's ARN interpolated and resourceId set to that ARN.
Traces: pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle#task#6 (GetSubscription) and pricing-plan-manager-subscription-mutations-on-unknown-well-formed-arn#task#1 (CancelSubscription) — the same wording defect in two handlers.

Cause 2 of 4: a malformed subscription ARN is treated as a missing subscription — 404 ResourceNotFoundException instead of 400 ValidationException.
AWS: Smithy: GetSubscription.arn targets SubscriptionArn (@length 1..2048, @arnReference AWS::PricingPlanManager::Subscription) and the operation models both ValidationException (400) and ResourceNotFoundException (404). An ARN that is not a well-formed subscription ARN fails input validation before any lookup, so AWS answers HTTP 400 with Error.Code "ValidationException" and Error.Message "Invalid subscription ARN: <arn>" — fixed text "Invalid subscription ARN: " plus the arn from the request. This corpus pins the syntax boundary from both sides: the region-less colon form arn:aws:pricingplanmanager::<account>:subscription:sub_<id> is well formed and answered 404 not-found, while the '/'-separated form with a non-'sub_' id is answered 400 invalid-ARN. The check applies to every operation that accepts a subscription ARN and runs ahead of that operation's other inputs.
Emulator: HTTP 404 with ResourceNotFoundException and "no subscription with ARN <arn>" — it goes straight from the store lock to a map lookup (src/ops/get_subscription.rs, and src/state.rs:352 subscription()/:394 resolve_for_mutation for the mutators) with no ARN well-formedness check anywhere ahead of it.
Why: a client cannot distinguish its own ARN-construction bug from a subscription that was cancelled or never existed. It treats the 404 as "gone, carry on" (and SDK retry/branching on ResourceNotFoundException differs from ValidationException) when AWS is telling it the request is malformed. The open issue 20261109T121305Z-pricing-plan-manager-2-net-new-root-causes.md describes this same gate as its Cause 2, but it is a behavioral finding that carries no wire replay key, so this fixture step is not covered by it.
Fix: shared subscription-ARN validation in crate::state (an ARN parser/validator beside is_minted_subscription_arn in src/state.rs), invoked at the top of each ARN-taking handler ahead of the store lookup and ahead of ifMatch/resourceArns handling, returning the modeled ValidationException (400) with the message above; the existing ResourceNotFoundException path stays for well-formed ARNs that name nothing. Note this validator must accept the ARN shape the service mints, which is itself wrong today (filed under the subscription-representation issue).
Traces: pricing-plan-manager-subscription-arn-validated-before-mutation#task#1.

Cause 3 of 4: ListSubscriptions refuses an invalid nextToken with its own sentence instead of AWS's.
AWS: Smithy: ListSubscriptions.nextToken is a plain String, @paginated inputToken; the operation models ValidationException (400). Recorded behavior: a nextToken the service did not issue is refused HTTP 400 with Error.Message "The nextToken provided is invalid." — the whole sentence is fixed text and names no value from the request. Status and class already match.
Emulator: "nextToken is not a token this service issued" — src/state.rs:573-578 decode_page_token() returns None for an unrecognised token and ListSubscriptions' handler renders that wording.
Why: pagination error handling is commonly matched on this exact AWS sentence (it is the same string AWS uses across services), so a client that detects a stale or corrupted cursor by message fails to recognise it and either loops or aborts the page walk.
Fix: ListSubscriptions' validation message in its handler (src/ops/list_subscriptions.rs, the decode_page_token failure arm) — return the modeled ValidationException (400) with AWS's sentence verbatim.
Traces: pricing-plan-manager-list-subscriptions-empty-and-invalid-token#task#2.

Cause 4 of 4: a stale If-Match on CancelSubscription is refused under the wrong class, status and message — 409 ConflictException instead of 400 ValidationException.
AWS: Smithy: CancelSubscription.ifMatch is @required and bound to the If-Match header; the operation models both ValidationException (400) and ConflictException (409). Recorded behavior for an ETag that is not the current one: HTTP 400, Error.Code "ValidationException", Error.Message "Invalid or outdated ETag '<etag>': retrieve current ETag and retry." — fixed text "Invalid or outdated ETag '" and "': retrieve current ETag and retry.", with the ETag echoed from the request's If-Match header. Hypothesis for the split against ConflictException's own documentation (which mentions If-Match generically): a value that is not a currently-issued ETag is an input validation failure, while ConflictException is reserved for a genuine state conflict; the recording is what fixes the class. The refusal must be inert — the subscription keeps its status, carries no scheduledChange and returns the same eTag.
Emulator: HTTP 409, Error.Code "ConflictException", Error.Message "the If-Match header does not match the current version of the subscription; retrieve the latest version and retry" — wrong status, wrong code, and a message that neither matches AWS's wording nor echoes the offending ETag. src/ops/cancel_subscription.rs maps state::PreconditionFailure::ETagMismatch (src/state.rs:316) onto ConflictException with state::IF_MATCH_CONFLICT_MESSAGE (src/state.rs:325-327).
Why: the class drives client behavior. A 409 ConflictException reads as a retryable optimistic-concurrency collision, so a client loops on it or fails to match the error code it was told to expect, where AWS reports a 400 the client must fix by re-reading the ETag; and the logged message never names the ETag that was rejected. The open issue 20261111T004423Z-cloudfront-pricing-plan-manager-wafv2-4-net-new.md states this as its Cause 3 but is behavioral and carries no wire replay key, so this fixture step is not covered by it.
Fix: the ETagMismatch refusal mapping in src/ops/cancel_subscription.rs and the shared message constant it uses in src/state.rs — return the modeled ValidationException (400) with the sentence above, interpolating the request's If-Match value, leaving the record and its eTag untouched; apply the same mapping in the other If-Match-bearing mutators so the class is consistent.
Traces: pricing-plan-manager-free-subscription-token-replay-and-etag-rules#task#12.

## Evidence
Replay of trace set simparity-wire-aws-v1. ARNs, ids and ETag values below are per-run values filling the templates; the fixed text of each message is the requirement.

Cause 1 — fixture pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle step task#6 (GetSubscription, 404 vs 404):
  $.message: expected "Subscription not found: arn:aws:pricingplanmanager::000000000000:subscription:sub_sim2realprobemissing00. Verify the ARN is correct and the subscription exists in your account.", got "no subscription with ARN arn:aws:pricingplanmanager::000000000000:subscription:sub_sim2realprobemissing00"
  Same defect on the mutation path — fixture pricing-plan-manager-subscription-mutations-on-unknown-well-formed-arn step task#1 (CancelSubscription, 404 vs 404): expected "Subscription not found: <arn>. Verify the ARN is correct and the subscription exists in your account.", got "no subscription with ARN <arn>".
  Smithy: ResourceNotFoundException httpError 404, required message + resourceId.

Cause 2 — fixture pricing-plan-manager-subscription-arn-validated-before-mutation step task#1 (GetSubscription, expected 400, got 404):
  status: expected 400, got 404
  $.message: expected "Invalid subscription ARN: arn:aws:pricingplanmanager::000000000000:subscription/sim2real-probe-ppm-missing-151210ab", got "no subscription with ARN arn:aws:pricingplanmanager::000000000000:subscription/sim2real-probe-ppm-missing-151210ab"
  $headers.x-amzn-errortype: expected the ValidationException error type, got 'ResourceNotFoundException'
  Smithy: ValidationException httpError 400; GetSubscription models both 400 and 404.

Cause 3 — fixture pricing-plan-manager-list-subscriptions-empty-and-invalid-token step task#2 (ListSubscriptions, 400 vs 400):
  $.message: expected "The nextToken provided is invalid.", got "nextToken is not a token this service issued"
  Source: src/state.rs:573-578 decode_page_token.

Cause 4 — fixture pricing-plan-manager-free-subscription-token-replay-and-etag-rules step task#12 (CancelSubscription, expected 400, got 409):
  status: expected 400, got 409
  $.message: expected "Invalid or outdated ETag 'sim2real-stale-etag': retrieve current ETag and retry." (fixed text + the If-Match value from the request), got "the If-Match header does not match the current version of the subscription; retrieve the latest version and retry"
  $headers.x-amzn-errortype: expected the ValidationException error type, got 'ConflictException'
  Source: src/ops/cancel_subscription.rs (ETagMismatch arm), src/state.rs:316, :325-327.

## Steps to Reproduce
Point a client at the emulator (region us-east-1, pricing-plan-manager endpoint).

Cause 1:
1. GetSubscription(arn="arn:aws:pricingplanmanager::000000000000:subscription:sub_doesnotexist000000001") — a well-formed but absent ARN.
2. Observe 404 with "no subscription with ARN <arn>"; AWS answers 404 with "Subscription not found: <arn>. Verify the ARN is correct and the subscription exists in your account."
3. Repeat with CancelSubscription(arn=<same>, ifMatch="1") — same wording defect.

Cause 2:
1. GetSubscription(arn="arn:aws:pricingplanmanager::000000000000:subscription/not-a-subscription-id") — '/' separator, no 'sub_' id, i.e. malformed.
2. Observe HTTP 404 ResourceNotFoundException; AWS answers HTTP 400 ValidationException "Invalid subscription ARN: <arn>".
3. Repeat with CancelSubscription / CancelSubscriptionChange / AssociateResourcesToSubscription / DisassociateResourcesFromSubscription — each must give the identical 400/ValidationException/message, and must do so before validating its other inputs.

Cause 3:
1. ListSubscriptions(nextToken="not-a-token").
2. Observe 400 "nextToken is not a token this service issued"; AWS answers 400 "The nextToken provided is invalid."

Cause 4:
1. CreateSubscription(planFamily="CloudFront", planTier="FREE", resourceArns=[<dist ARN>, <acl ARN>]); poll to ACTIVE.
2. CancelSubscription(arn=<sub arn>, ifMatch="some-stale-etag").
3. Observe HTTP 409 ConflictException with the emulator's sentence; AWS answers HTTP 400 ValidationException "Invalid or outdated ETag 'some-stale-etag': retrieve current ETag and retry."
4. Confirm inertness: GetSubscription still reports the prior status, no scheduledChange, and the unchanged eTag.

Replay: replay_wire_corpus and re-run pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle#task#6, pricing-plan-manager-subscription-mutations-on-unknown-well-formed-arn#task#1, pricing-plan-manager-subscription-arn-validated-before-mutation#task#1, pricing-plan-manager-list-subscriptions-empty-and-invalid-token#task#2 and pricing-plan-manager-free-subscription-token-replay-and-etag-rules#task#12.
