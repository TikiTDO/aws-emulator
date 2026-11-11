Issue-Source: trace-sweep-divergence
Issue-Trace: pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle pricing-plan-manager-free-subscription-hosted-zone-association pricing-plan-manager-free-subscription-token-replay-and-etag-rules
Issue-Shape-Id: com.amazonaws.pricingplanmanager#AWSPricingPlanManager
Issue-Key: pricing-plan-manager|CreateSubscription|2xx->2xx|$.arn
Issue-Key: pricing-plan-manager|CreateSubscription|2xx->2xx|$.createdAt
Issue-Key: pricing-plan-manager|CreateSubscription|2xx->2xx|$.updatedAt
Issue-Key: pricing-plan-manager|GetSubscription|2xx->2xx|$.createdAt
Issue-Key: pricing-plan-manager|GetSubscription|2xx->2xx|$.updatedAt
Issue-Key: pricing-plan-manager|ListSubscriptions|2xx->2xx|$.subscriptionSummaries[arn="arn:aws:pricingplanmanager:us-east-1:000000000000:subscription/e4e1b77f-7f80-cd36-1d80-f3b9bd2d7903"].createdAt
Issue-Key: pricing-plan-manager|ListSubscriptions|2xx->2xx|$.subscriptionSummaries[arn="arn:aws:pricingplanmanager:us-east-1:000000000000:subscription/e4e1b77f-7f80-cd36-1d80-f3b9bd2d7903"].eTag
Issue-Key: pricing-plan-manager|ListSubscriptions|2xx->2xx|$.subscriptionSummaries[arn="arn:aws:pricingplanmanager:us-east-1:000000000000:subscription/e4e1b77f-7f80-cd36-1d80-f3b9bd2d7903"].updatedAt
Issue-Key: pricing-plan-manager|GetSubscription|2xx->2xx|$.resourceArns[*]
Issue-Key: pricing-plan-manager|ListSubscriptions|2xx->2xx|$.subscriptionSummaries[*].createdAt
Issue-Key: pricing-plan-manager|ListSubscriptions|2xx->2xx|$.subscriptionSummaries[*].resourceArns[*]
Issue-Key: pricing-plan-manager|ListSubscriptions|2xx->2xx|$.subscriptionSummaries[*].updatedAt
Issue-Commit: af64552852b9de70172a1fcfdec68b8a2bd1c743

# Subscription representation diverges: epoch-second timestamps, mis-minted subscription ARNs, digest eTags and a dropped resource ARN

> _Note: this issue was identified against the code base at commit `af64552852b9de70172a1fcfdec68b8a2bd1c743`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSubscription

## Description
Four net-new root causes in how this service renders a subscription on the wire. Every one of them is visible on an operation that answers 200, so a client is not warned: it simply reads values whose type or syntax AWS never produces. None is marked ALREADY FILED or OWNED in the replay report, and none is covered by the open pricing-plan-manager issues (20261109T121305Z-pricing-plan-manager-2-net-new-root-causes.md, 20261111T004423Z-cloudfront-pricing-plan-manager-wafv2-4-net-new.md, 20261109T121305Z-cloudfront-pricing-plan-manager-wafv2-createsubscription-does-not.md), which all describe refusal gates rather than response rendering.

Cause 1 of 4: createdAt / updatedAt (and ScheduledChange.effectiveDate) are serialized as epoch-second numbers instead of ISO-8601 date-time strings.
AWS: Smithy (services/service-com-amazonaws-pricingplanmanager-awspricingplanmanager/smithy-model/com-amazonaws-pricingplanmanager-awspricingplanmanager.json): Subscription.createdAt and Subscription.updatedAt are @required Timestamp members carrying @timestampFormat("date-time"), documented as "The date and time when the subscription was created, in ISO 8601 format"; SubscriptionSummary.createdAt/updatedAt and ScheduledChange.effectiveDate carry the same trait and the same "in ISO 8601 format" wording. So on restJson1 these members are strings of the form YYYY-MM-DDThh:mm:ss(.sss)Z, not JSON numbers. The instant itself is a per-run value and is not part of the requirement; its wire TYPE and format are.
Emulator: every Subscription / SubscriptionSummary / ScheduledChange it serializes carries these members as JSON numbers of epoch seconds with a fractional part — the replay reports createdAt/updatedAt as 1794393983.569 and effectiveDate as 1796083200 where the recording holds an ISO-8601 string. The members are typed ::aws_smithy_types::DateTime in src/ops/types/shared_types.rs (Subscription lines 25-26, ScheduledChange line 33) and in src/ops/types/list_subscriptions_types.rs, and the derived serializer renders them in the restJson1 default epoch-seconds form instead of honouring the model's @timestampFormat("date-time"). The service already owns the correct renderer: state.rs:473 format_date_time() emits the RFC 3339 date-time.
Why: an SDK that deserializes a modeled date-time member gets a type error or a nonsense date for every read of a subscription — CreateSubscription, GetSubscription, ListSubscriptions, CancelSubscription, UpdateSubscription and ApprovePaidSubscription all return a Subscription. A client comparing createdAt to a datetime, formatting it, or round-tripping it into another call cannot use the value at all, and this is the single most-replayed divergence in the corpus (it appears in 13 of the 78 replayed steps).
Fix: the response serializer for the subscription shapes — the smithy_macros::smithy derive on Subscription / ScheduledChange in src/ops/types/shared_types.rs and on SubscriptionSummary in src/ops/types/list_subscriptions_types.rs must emit these Timestamp members in the model's date-time format (state.rs format_date_time is the existing renderer). This is one serializer change, not a per-handler one.
Traces: pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle#task#7 (CreateSubscription), #task#8 (GetSubscription), #task#9 (ListSubscriptions); pricing-plan-manager-free-subscription-token-replay-and-etag-rules#task#6.

Cause 2 of 4: the subscription ARN the service mints has the wrong shape — it carries a region, uses a '/' separator and a UUID-shaped id, where AWS's subscription ARN is region-less with a ':subscription:sub_<id>' resource part.
AWS: Smithy: Subscription.arn targets SubscriptionArn (@length 1..2048, @arnReference AWS::PricingPlanManager::Subscription); the model fixes no literal syntax, so this is recorded behavior only. Across the corpus every subscription ARN AWS accepts as well-formed has the form arn:aws:pricingplanmanager::<account-id>:subscription:sub_<id> — empty region segment (the resource is account-global) and a colon-separated 'subscription' resource type whose id is prefixed 'sub_'. The corroborating negative is in this same corpus: an ARN written with the '/' separator and a non-'sub_' id is refused 400 "Invalid subscription ARN: <arn>" (fixture pricing-plan-manager-subscription-arn-validated-before-mutation#task#1) while the region-less ':subscription:sub_...' form is accepted and answered 404 "Subscription not found: ...". The <id> value itself is per-run and not a requirement; the three structural elements are.
Emulator: CreateSubscription mints arn:aws:pricingplanmanager:us-east-1:<account>:subscription/<8-4-4-4-12 hex> — region populated, '/' separator, no 'sub_' prefix. state.rs:523-547 mint_subscription_arn() builds format!("subscription/{id}") from a hex digest and passes the request region to engine::emulator_arn, and state.rs is_minted_subscription_arn/decode_page_token are written around that same shape.
Why: an ARN is a contract, not a cosmetic string. A client that stores the emulator's ARN and later hands it to AWS (or to the ARN validation AWS applies, which this corpus shows refuses the '/' form) gets a 400 ValidationException for an ARN the emulator itself issued; cross-service code that parses the ARN reads a region for a global resource and a resource id it cannot match against the 'sub_' form, and IAM-style resource patterns written for AWS ARNs never match the emulator's.
Fix: ARN minting in state.rs mint_subscription_arn() (and the matching recogniser is_minted_subscription_arn plus the page-token codec that depends on it) — mint arn:aws:pricingplanmanager::<account>:subscription:sub_<id> with an empty region segment, and keep the validator in sync so the service accepts the ARNs it issues.
Traces: pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle#task#7 ($.arn shape mismatch on CreateSubscription); the negative control is pricing-plan-manager-subscription-arn-validated-before-mutation#task#1, filed separately.

Cause 3 of 4: SubscriptionSummary.eTag is a 128-bit hex digest instead of the small decimal revision AWS publishes.
AWS: Smithy: SubscriptionSummary.eTag is a @required String, "The entity tag for concurrency control. Pass this value in the If-Match header when making changes to this subscription." The format is recorded behavior only: in the recording the summary for a subscription that has been mutated once carries eTag "2" — a short decimal integer. Hypothesis: AWS issues a monotonic per-subscription version number that starts at 1 and increments on each mutation, which is consistent with the one recorded value. What is certain and asserted is that the value is a small decimal integer, not a digest.
Emulator: ListSubscriptions reports eTag "0b2964f7afc92265cb55210f6565c70b" for the same subscription — hex of the first 16 bytes of a SHA-256 over the stored record (state.rs:216-251 SubscriptionRecord::etag(), surfaced by to_summary() at state.rs:283). The record already keeps the counter this needs: SubscriptionRecord.revision (state.rs:205), bumped by touch() (state.rs:290-293).
Why: the eTag is the token a client echoes in If-Match, and clients (and recorded request fixtures) do compare, log and sometimes construct it. A caller that has a stored AWS-shaped eTag, or that asserts on the concurrency token's form, diverges on every list; and because the digest changes shape as well as value, no test written against AWS's tokens can be replayed against the emulator.
Fix: the eTag derivation in state.rs (SubscriptionRecord::etag) — publish the record's revision counter as a decimal string instead of the digest, so to_summary() and the ETag response header both carry it; resolve_for_mutation's If-Match comparison (state.rs:394-407) then compares the same value.
Traces: pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle#task#9.

Cause 4 of 4: GetSubscription and ListSubscriptions report only two of the three resource ARNs the subscription covers — the Route 53 hosted zone added to it is missing from resourceArns.
AWS: Smithy: Subscription.resourceArns / SubscriptionSummary.resourceArns are @required ResourceArns (@length 1..10), "The ARNs of the resources covered by this subscription". AWS reports every covered resource, so after a hosted zone is added to a CloudFront-family subscription the reads list three ARNs (distribution, web ACL, hosted zone); the recording holds arn:aws:route53:::hostedzone/<zone-id> at resourceArns[2]. The zone id is a per-run value; the fact that an added resource appears in resourceArns is the requirement.
Emulator: after the same sequence, GetSubscription and ListSubscriptions return a subscription whose resourceArns has only the two original entries — resourceArns[2] is absent. Hypothesis: this is the read-side symptom of the unimplemented AssociateResourcesToSubscription scaffold (its own issue is already filed as 20261110T215352Z-pricing-plan-manager-associateresourcestosubscription-stub.md), since nothing else in the recorded sequence adds the zone; the association never reaches state, so the reads are short. It is reported here because the diverging steps are these two 200-status reads, and because a fixer closing the stub must re-replay them to confirm the covered-resource list is actually reported.
Why: resourceArns is how a client discovers what the flat-rate plan covers. A reconciliation or billing-attribution pass reading the emulator believes the hosted zone is not covered, so it either re-subscribes it (which AWS refuses as already-subscribed) or prices it as pay-as-you-go.
Fix: AssociateResourcesToSubscription's handler plus the subscription record's resource_arns in crate::state (state.rs SubscriptionRecord.resource_arns, line 190) — the association must persist into the record so the shared to_subscription()/to_summary() conversions report it; no change is needed in the readers themselves.
Traces: pricing-plan-manager-free-subscription-hosted-zone-association#task#10 (GetSubscription), #task#11 (ListSubscriptions).

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 46.2%, 42 mismatched steps of 78). Per-run values below — instants, hex ids, zone ids, account 000000000000 — fill the templates and are not part of any requirement.

Cause 1 — fixture pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle step task#7 (CreateSubscription, 200 vs 200):
  $.createdAt: expected an ISO-8601 date-time string, got 1794393983.569
  $.updatedAt: expected an ISO-8601 date-time string, got 1794393983.569
  Same keys, same cause at #task#8 (GetSubscription, cluster of 5 steps across 3 fixtures), #task#9 (ListSubscriptions, inside subscriptionSummaries[...]), #task#10 (CancelSubscription, where $.scheduledChange.effectiveDate is also a number), and pricing-plan-manager-free-subscription-token-replay-and-etag-rules#task#6 (cluster of 2).
  Smithy: Subscription.createdAt/updatedAt @required Timestamp @timestampFormat("date-time"), doc "in ISO 8601 format". Source: src/ops/types/shared_types.rs:25-26, 33; renderer available at src/state.rs:473.

Cause 2 — same step task#7:
  $.arn: expected the recorded subscription-ARN shape arn:aws:pricingplanmanager::<account>:subscription:sub_<id>, got 'arn:aws:pricingplanmanager:us-east-1:000000000000:subscription/e4e1b77f-7f80-cd36-1d80-f3b9bd2d7903'.
  Corroboration inside this corpus: the region-less colon form is treated by AWS as well-formed-but-absent (404 "Subscription not found: arn:aws:pricingplanmanager::000000000000:subscription:sub_sim2realprobemissing00. ..."), while the '/' form is refused 400 "Invalid subscription ARN: ...".
  Source: src/state.rs:523-547 mint_subscription_arn().

Cause 3 — step task#9 (ListSubscriptions, 200 vs 200):
  $.subscriptionSummaries[arn=...].eTag: expected '2', got '0b2964f7afc92265cb55210f6565c70b'.
  Source: src/state.rs:216-251 (SHA-256 digest), surfaced at :283; the counter that should back it is SubscriptionRecord.revision at :205.

Cause 4 — fixture pricing-plan-manager-free-subscription-hosted-zone-association:
  step task#10 (GetSubscription, 200 vs 200): $.resourceArns[2]: expected 'arn:aws:route53:::hostedzone/ZYOYB5QXI3X90GN', got None.
  step task#11 (ListSubscriptions, 200 vs 200): $.subscriptionSummaries[0].resourceArns[2]: expected the same hosted-zone ARN, got None.
  Related already-filed stub: 20261110T215352Z-pricing-plan-manager-associateresourcestosubscription-stub.md.

## Steps to Reproduce
Point a client at the emulator's data listener (region us-east-1, pricing-plan-manager / pricingplanmanager endpoint).

Causes 1-3:
1. CreateSubscription(planFamily="CloudFront", planTier="FREE", resourceArns=[<a real CloudFront distribution ARN>, <its CLOUDFRONT-scope wafv2 web ACL ARN>]).
2. Inspect the 200 body: createdAt/updatedAt come back as JSON numbers (epoch seconds with a fraction) — AWS sends ISO-8601 date-time strings — and subscription.arn carries a region and a 'subscription/<uuid>' resource part instead of the region-less 'subscription:sub_<id>' form.
3. GetSubscription(arn=<that arn>) — same timestamp types.
4. ListSubscriptions() — subscriptionSummaries[].createdAt/updatedAt are numbers and subscriptionSummaries[].eTag is a 32-char hex digest where AWS publishes a small decimal revision (recorded: "2").

Cause 4:
1. CreateSubscription as above, poll GetSubscription to ACTIVE.
2. AssociateResourcesToSubscription(arn=<sub arn>, resourceArns=["arn:aws:route53:::hostedzone/<zone id>"], ifMatch=<current eTag>).
3. GetSubscription(arn) and ListSubscriptions(): resourceArns still lists only the two original ARNs; AWS lists three, with the hosted zone at resourceArns[2].

Replay: replay_wire_corpus and re-run fixture pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle steps task#7/task#8/task#9, pricing-plan-manager-free-subscription-token-replay-and-etag-rules step task#6, and pricing-plan-manager-free-subscription-hosted-zone-association steps task#10/task#11.
