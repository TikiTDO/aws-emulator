Issue-Source: trace-sweep-divergence
Issue-Trace: pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle
Issue-Shape-Id: com.amazonaws.pricingplanmanager#AWSPricingPlanManager
Issue-Key: pricing-plan-manager|ListSubscriptions|2xx->2xx|$.subscriptionSummaries[arn="arn:aws:pricingplanmanager:us-east-1:000000000000:subscription/6f5f2d53-964d-a203-0469-71991f199a0d"].createdAt
Issue-Key: pricing-plan-manager|ListSubscriptions|2xx->2xx|$.subscriptionSummaries[arn="arn:aws:pricingplanmanager:us-east-1:000000000000:subscription/6f5f2d53-964d-a203-0469-71991f199a0d"].eTag
Issue-Key: pricing-plan-manager|ListSubscriptions|2xx->2xx|$.subscriptionSummaries[arn="arn:aws:pricingplanmanager:us-east-1:000000000000:subscription/6f5f2d53-964d-a203-0469-71991f199a0d"].updatedAt
Issue-Commit: 692b7cc542e0c22fd13490bec007545cbb7cb62f

# ListSubscriptions eTag digest and epoch-number timestamps break SubscriptionSummary wire contract

> _Note: this issue was identified against the code base at commit `692b7cc542e0c22fd13490bec007545cbb7cb62f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSubscriptions

## Description
ListSubscriptions answers the modeled 200, so nothing warns the caller, but the SubscriptionSummary it renders for an already-mutated subscription carries two members in a wire form AWS never produces: the concurrency token (eTag) and the two date-time members (createdAt/updatedAt). The replay reports this cluster with an EMPTY owned_by and an empty stub_path, i.e. no open issue carries its replay key, so it is filed on its own.

Two open issues describe the same symptom family but do NOT cover this step, because their Issue-Key lines are pinned to a previous run's per-run minted subscription ARN and therefore no longer join to this cluster's key: 20261111T140419Z-pricing-plan-manager-listsubscriptions-renders-subscription-summaries-with-digest-etags-and.md (keys pinned to arn=...96c5a7f8-...) and 20261111T215818Z-pricing-plan-manager-subscriptionsummary-concurrency-token-and-date-time-members-are.md (keys pinned to arn=...df14b601-...). A third, 20261111T105841Z-pricing-plan-manager-subscription-representation-diverges-epoch-second-timestamps-mis-minted.md, owns the Get/Create representation clusters, not the list-summary path. The key-pinning to a per-run ARN is itself why this keeps re-surfacing unowned: the fix must be made in the shared summary serializer / state so the whole cluster stops diverging, not pinned to one recorded ARN.

Cause 1 of 2: SubscriptionSummary.eTag is published as a 128-bit hex digest instead of the small decimal revision AWS publishes. Fixture pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle, step task#9.
AWS: Smithy (services/service-com-amazonaws-pricingplanmanager-awspricingplanmanager/smithy-model/com-amazonaws-pricingplanmanager-awspricingplanmanager.json), shape SubscriptionSummary: member eTag -> String, @required, doc "The entity tag for concurrency control. Pass this value in the If-Match header when making changes to this subscription." The model fixes no literal syntax, so the format is recorded behavior only: the recording for a subscription that has taken one mutation after creation carries eTag "2" -- a short decimal integer. Hypothesis: AWS issues a monotonic per-subscription revision that starts at 1 and increments on each mutation, consistent with the single recorded value. What is asserted is the FORM (a small decimal integer, not a digest); the particular integer is a function of how many mutations the subscription has seen and is not itself a requirement.
Emulator: the summary for the same subscription carries a 32-character lowercase hex digest -- a content hash over the stored record -- where the recording holds a decimal revision (this run: '34c2f41f8bb2c58798c973743c077d6c' vs expected '2'; the digest value is per-run).
Why: the eTag is the exact token a client echoes in If-Match on CancelSubscription and other mutations. A caller that persists an AWS-shaped token, compares the token it listed against the one it stored, or asserts on the token's form diverges on every list, and no optimistic-concurrency test written against AWS can be replayed against the emulator because the token's shape differs, not just its value.
Fix: the eTag derivation on the subscription state record and the to_summary conversion that surfaces it into the ListSubscriptions output (the service crate's state module / summary conversion) -- publish the record's existing revision counter as a decimal string, and keep the If-Match comparison on the mutation path reading the same value.
Traces: pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle#task#9.

Cause 2 of 2: SubscriptionSummary.createdAt / updatedAt are serialized as epoch-second JSON numbers instead of ISO-8601 date-time strings. Fixture pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle, step task#9.
AWS: Smithy: SubscriptionSummary.createdAt and SubscriptionSummary.updatedAt are @required Timestamp members with timestampFormat "date-time", documented "The date and time when the subscription was created / was last modified, in ISO 8601 format." On restJson1 that is a JSON STRING of the form YYYY-MM-DDThh:mm:ss(.sss)Z. The instant itself is per-run and is not part of the requirement; the wire TYPE and format are.
Emulator: both members are emitted as JSON numbers of epoch seconds with a fractional part (this run: 1794523802.76) -- the restJson1 default timestamp encoding -- ignoring the member's timestampFormat "date-time".
Why: an SDK deserializing a modeled date-time member against a JSON number gets a type error or a nonsense instant for every listed subscription, so reconciliation or pagination code that sorts/filters on createdAt/updatedAt cannot use the value at all.
Fix: the response serializer for the list shapes -- the SubscriptionSummary serializer used by the ListSubscriptions output types must render these Timestamp members in the model's date-time format, reusing the service's RFC 3339 renderer. One serializer change, not a per-handler one.
Traces: pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle#task#9.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up=true, bootstrap_signal=false, score_pct=46.2, steps_total=78, steps_passed=36, steps_mismatched=42, divergence_count=16. The concrete values quoted below are per-run (a minted subscription ARN, a hex digest, instants, account 000000000000) and only fill the templates; they are not themselves requirements.

Divergence cluster (index 4), verdict=mismatch, service=pricing-plan-manager, operation=ListSubscriptions, fixture_id=pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle, step_key=task#9, expected_status=200, actual_status=200, affected_count=1, owned_by=[] (NOT already filed), stub_path="":
  divergence_key: service=pricing-plan-manager|ListSubscriptions|status=200->200|keys=$.subscriptionSummaries[arn="..."].createdAt,$.subscriptionSummaries[arn="..."].eTag,$.subscriptionSummaries[arn="..."].updatedAt
  $.subscriptionSummaries[arn="arn:aws:pricingplanmanager:us-east-1:000000000000:subscription/6f5f2d53-964d-a203-0469-71991f199a0d"].eTag: value_mismatch -- expected '2' (a small decimal revision), got '34c2f41f8bb2c58798c973743c077d6c' (a 32-character hex digest).
  $.subscriptionSummaries[arn="...same arn..."].createdAt: value_mismatch -- expected an ISO-8601 date-time string (asserted as shaped, not literal: '<shaped-not-asserted>'), got the JSON number 1794523802.76.
  $.subscriptionSummaries[arn="...same arn..."].updatedAt: value_mismatch -- expected an ISO-8601 date-time string ('<shaped-not-asserted>'), got the JSON number 1794523802.76.

Model evidence (smithy_query query=shape shape=SubscriptionSummary, model_path services/service-com-amazonaws-pricingplanmanager-awspricingplanmanager/smithy-model/com-amazonaws-pricingplanmanager-awspricingplanmanager.json): eTag -> String, required=true, doc "The entity tag for concurrency control. Pass this value in the If-Match header when making changes to this subscription."; createdAt -> Timestamp, required=true, timestampFormat "date-time", doc "The date and time when the subscription was created, in ISO 8601 format."; updatedAt -> Timestamp, required=true, timestampFormat "date-time", doc "The date and time when the subscription was last modified, in ISO 8601 format."

No status or error-code divergence is involved: the operation answers the modeled 200 and the rest of the body shape matches, so this is purely a response-encoding/state-representation defect.

## Steps to Reproduce
Point a client at the emulator's data listener (region us-east-1, pricingplanmanager endpoint).
1. CreateSubscription(planFamily="CloudFront", planTier="FREE", resourceArns=[<a CloudFront distribution ARN>, <its CLOUDFRONT-scope wafv2 web ACL ARN>]).
2. Poll GetSubscription(arn=<the minted ARN>) until status is ACTIVE -- i.e. let the subscription take at least one state mutation after creation.
3. ListSubscriptions().
4. Inspect the returned subscriptionSummaries entry for that ARN: eTag is a 32-character hex digest where AWS publishes a small decimal revision (the recording holds "2" for a once-mutated subscription), and createdAt/updatedAt are JSON numbers of epoch seconds where AWS sends ISO-8601 date-time strings.

Replay: call replay_wire_corpus and re-run fixture pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle, step task#9.
