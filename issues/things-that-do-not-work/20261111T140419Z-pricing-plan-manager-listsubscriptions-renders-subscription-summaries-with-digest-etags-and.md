Issue-Source: trace-sweep-divergence
Issue-Trace: pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle
Issue-Shape-Id: com.amazonaws.pricingplanmanager#AWSPricingPlanManager
Issue-Key: pricing-plan-manager|ListSubscriptions|2xx->2xx|$.subscriptionSummaries[arn="arn:aws:pricingplanmanager:us-east-1:000000000000:subscription/96c5a7f8-defa-47fe-8ba3-f6f3b91f81dc"].createdAt
Issue-Key: pricing-plan-manager|ListSubscriptions|2xx->2xx|$.subscriptionSummaries[arn="arn:aws:pricingplanmanager:us-east-1:000000000000:subscription/96c5a7f8-defa-47fe-8ba3-f6f3b91f81dc"].eTag
Issue-Key: pricing-plan-manager|ListSubscriptions|2xx->2xx|$.subscriptionSummaries[arn="arn:aws:pricingplanmanager:us-east-1:000000000000:subscription/96c5a7f8-defa-47fe-8ba3-f6f3b91f81dc"].updatedAt
Issue-Commit: 9b0642efcbb416a2d848390c0f01f1a772175058

# ListSubscriptions renders subscription summaries with digest eTags and epoch-second timestamps

> _Note: this issue was identified against the code base at commit `9b0642efcbb416a2d848390c0f01f1a772175058`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSubscriptions

## Description
Two net-new root causes in how ListSubscriptions renders a SubscriptionSummary for a subscription that has already been mutated. The step answers 200, so a client is never warned: it simply reads a concurrency token and two date-time members whose wire form AWS never produces. This cluster is reported by the replay with an EMPTY owned_by, so no open issue carries its replay key; the closest open issue is 20261111T105841Z-pricing-plan-manager-subscription-representation-diverges-epoch-second-timestamps-mis-minted.md, whose Issue-Keys for this path are pinned to a per-run minted subscription ARN (arn=...e4e1b77f-...) and therefore no longer match this cluster's key (arn=...96c5a7f8-...). It must be re-replayed and closed on its own.

Cause 1 of 2: SubscriptionSummary.eTag is a 128-bit hex digest instead of the small decimal revision AWS publishes.
AWS: Smithy (services/service-com-amazonaws-pricingplanmanager-awspricingplanmanager/smithy-model/com-amazonaws-pricingplanmanager-awspricingplanmanager.json): SubscriptionSummary.eTag is a @required String documented "The entity tag for concurrency control. Pass this value in the If-Match header when making changes to this subscription." The model fixes no literal syntax, so the format is recorded behavior only: in the recording the summary of a subscription that has been mutated once carries eTag "2" — a short decimal integer. Hypothesis: AWS issues a monotonic per-subscription version that starts at 1 and increments on each mutation, which is consistent with the one recorded value. What is asserted and certain is that the value is a small decimal integer, not a digest; the particular integer is a function of how many mutations the subscription has seen, not a literal requirement.
Emulator: ListSubscriptions reports a 32-character lowercase hex digest for the same subscription — a content hash over the stored record rather than a revision counter (the replay shows '4129d66a37067edc83116a16c2f2883a' where the recording holds '2').
Why: the eTag is the token a client echoes back in If-Match. A caller that holds an AWS-shaped token, that compares the token it listed against the one it stored, or that asserts on the concurrency token's form, diverges on every list; and because a digest changes shape as well as value, no optimistic-concurrency test written against AWS can be replayed against the emulator.
Fix: the eTag derivation in the subscription state record and the to_summary() conversion that surfaces it (crate::state) — publish the record's existing revision counter as a decimal string instead of the digest, and keep the If-Match comparison on the mutation path reading the same value.
Traces: pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle#task#9.

Cause 2 of 2: createdAt / updatedAt inside subscriptionSummaries are serialized as epoch-second JSON numbers instead of ISO-8601 date-time strings.
AWS: Smithy: SubscriptionSummary.createdAt and SubscriptionSummary.updatedAt are @required Timestamp members carrying @timestampFormat("date-time"), documented "The date and time when the subscription was created/was last modified, in ISO 8601 format". On restJson1 that means a JSON string of the form YYYY-MM-DDThh:mm:ss(.sss)Z. The instant itself is a per-run value and is not part of the requirement; the wire TYPE and format are.
Emulator: each summary carries these members as JSON numbers of epoch seconds with a fractional part (the replay reports 1794405478.433 where the recording holds a date-time string), i.e. the restJson1 default timestamp encoding instead of the model's @timestampFormat("date-time").
Why: an SDK deserializing a modeled date-time member gets a type error or a nonsense instant for every listed subscription, so pagination/reconciliation code that sorts or filters on createdAt/updatedAt cannot use the value at all.
Fix: the response serializer for the list shapes — the SubscriptionSummary type's serializer in the ListSubscriptions output types must emit these Timestamp members in the model's date-time format, reusing the service's existing RFC 3339 renderer in crate::state. One serializer change, not a per-handler one.
Traces: pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle#task#9.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: score 46.2%, 42 mismatched of 78 steps, 16 divergence clusters. The values quoted below are per-run (minted ARN, hex digest, instants, account 000000000000) and fill the templates; they are not themselves requirements.

Fixture pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle, step task#9 (ListSubscriptions, expected 200, actual 200), cluster key service=pricing-plan-manager|ListSubscriptions|status=200->200|keys=$.subscriptionSummaries[arn="..."].createdAt,$.subscriptionSummaries[arn="..."].eTag,$.subscriptionSummaries[arn="..."].updatedAt, owned_by: [] (NOT already filed), stub_path: "":
  $.subscriptionSummaries[arn="arn:aws:pricingplanmanager:us-east-1:000000000000:subscription/96c5a7f8-defa-47fe-8ba3-f6f3b91f81dc"].eTag: value_mismatch — expected '2' (a small decimal revision), got '4129d66a37067edc83116a16c2f2883a' (a 32-char hex digest).
  $.subscriptionSummaries[arn="...same arn..."].createdAt: value_mismatch — expected an ISO-8601 date-time string, got the JSON number 1794405478.433.
  $.subscriptionSummaries[arn="...same arn..."].updatedAt: value_mismatch — expected an ISO-8601 date-time string, got the JSON number 1794405478.433.

Model evidence (smithy_query shape=SubscriptionSummary): eTag -> String, @required, doc "The entity tag for concurrency control. Pass this value in the If-Match header when making changes to this subscription."; createdAt/updatedAt -> Timestamp, @required, timestampFormat "date-time", doc "...in ISO 8601 format."; ListSubscriptions is POST /v1/ListSubscriptions, 200, paginated on nextToken/subscriptionSummaries.

No status or error-code divergence is involved: the operation answers the modeled 200 and the body shape is otherwise correct.

## Steps to Reproduce
Point a client at the emulator's data listener (region us-east-1, pricingplanmanager endpoint).
1. CreateSubscription(planFamily="CloudFront", planTier="FREE", resourceArns=[<a CloudFront distribution ARN>, <its CLOUDFRONT-scope wafv2 web ACL ARN>]).
2. Poll GetSubscription(arn=<the minted arn>) until status is ACTIVE — i.e. let the subscription take at least one state mutation after creation.
3. ListSubscriptions().
4. Inspect subscriptionSummaries[0]: eTag is a 32-character hex digest where AWS publishes a small decimal revision (the recording holds "2" for a once-mutated subscription), and createdAt/updatedAt are JSON numbers of epoch seconds where AWS sends ISO-8601 date-time strings.

Replay: call replay_wire_corpus and re-run fixture pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle step task#9.
