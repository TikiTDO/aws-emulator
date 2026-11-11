Issue-Source: trace-sweep-divergence
Issue-Trace: pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle pricing-plan-manager-free-subscription-hosted-zone-association pricing-plan-manager-free-subscription-token-replay-and-etag-rules
Issue-Shape-Id: com.amazonaws.pricingplanmanager#AWSPricingPlanManager
Issue-Key: pricing-plan-manager|CancelSubscription|2xx->2xx|$.createdAt
Issue-Key: pricing-plan-manager|CancelSubscription|2xx->2xx|$.scheduledChange.effectiveDate
Issue-Key: pricing-plan-manager|CancelSubscription|2xx->2xx|$.updatedAt
Issue-Key: pricing-plan-manager|GetSubscription|4xx->2xx|$
Issue-Key: pricing-plan-manager|GetSubscription|4xx->2xx|$.message
Issue-Key: pricing-plan-manager|GetSubscription|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: pricing-plan-manager|GetSubscription|4xx->2xx|status
Issue-Commit: af64552852b9de70172a1fcfdec68b8a2bd1c743

# Cancelling a FREE subscription never completes and publishes an effectiveDate before the change is confirmed

> _Note: this issue was identified against the code base at commit `af64552852b9de70172a1fcfdec68b8a2bd1c743`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelSubscription

## Description
Two net-new root causes in the cancellation lifecycle. Both are 200-status answers, so nothing tells the client the subscription it asked to cancel is still billing. Neither is marked ALREADY FILED or OWNED in the replay report. The open issue 20261111T004423Z-cloudfront-pricing-plan-manager-wafv2-4-net-new.md mentions the post-cancel 404 only as an unreached follow-on hypothesis under its Cause 3 (the stale-ETag class) and carries no replay key for it; this issue is the wire-grounded report of the lifecycle itself.

Cause 1 of 2: CancelSubscription dates the scheduled cancellation in the response, where AWS leaves effectiveDate absent until the billing system confirms the change.
AWS: Smithy (services/service-com-amazonaws-pricingplanmanager-awspricingplanmanager/smithy-model/com-amazonaws-pricingplanmanager-awspricingplanmanager.json): ScheduledChange.effectiveDate is an OPTIONAL Timestamp (@timestampFormat date-time) whose own documentation states "The date and time when the change takes effect, in ISO 8601 format. This value is populated after the change is confirmed by the billing system." So the CancelSubscription response that first attaches the pending change reports scheduledChange with changeType "CANCELLATION" and NO effectiveDate member; the member appears only once the change is confirmed. The recording agrees: the member is absent in the recorded response.
Emulator: the CancelSubscription 200 body carries scheduledChange.effectiveDate already populated — the first instant of the next calendar month. src/state.rs:501-521 end_of_billing_period() is called at cancel time and stored into ScheduledChangeRecord.effective_date (src/state.rs:154), which to_shape() (src/state.rs:163-170) then publishes.
Why: a client reads a confirmed effective date that AWS has not issued and schedules work against it (teardown, budget cutover, alarms). Worse, that date is the emulator's own trigger for completing the cancellation (cause 2), so publishing it is not cosmetic: it is what defers the deletion.
Fix: CancelSubscription's handler (src/ops/cancel_subscription.rs, the arm that attaches the pending change) together with ScheduledChangeRecord in crate::state — attach the pending CANCELLATION with effective_date None, and populate it only when the change is confirmed, exactly as the member's documentation describes. state.rs apply_due_changes() already treats a change with no effective_date as "not dated yet" (src/state.rs:428-432), so the record shape is supported.
Traces: pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle#task#10.

Cause 2 of 2: the cancellation never completes — the subscription stays ACTIVE indefinitely, so GetSubscription never reaches the modeled not-found answer.
AWS: Smithy: GetSubscription models ResourceNotFoundException (httpError 404, @required members message + resourceId, doc "The specified subscription was not found"); CancelSubscription's own documentation says "For active subscriptions, the cancellation is scheduled to take effect at the end of the current billing period" and "For subscriptions in PENDING_APPROVAL status, the subscription is deleted immediately". Recorded behavior for the FREE CloudFront tier exercised here: after CancelSubscription the subscription is gone within the fixture's poll window and GetSubscription answers HTTP 404, Error.Code "ResourceNotFoundException", Error.Message "Subscription not found: <arn>. Verify the ARN is correct and the subscription exists in your account." — fixed text "Subscription not found: " plus ". Verify the ARN is correct and the subscription exists in your account.", with the arn copied from the request. Hypothesis: a FREE-tier subscription carries no billing commitment, so its cancellation is processed promptly rather than held to a month boundary; what the recording fixes is that the terminal state is reached (and reached quickly), not the exact delay.
Emulator: GetSubscription keeps answering HTTP 200 for the full poll (15 attempts) with status "ACTIVE" and the pending scheduledChange attached. src/state.rs:409-451 apply_due_changes() only removes a cancelled record once the clock passes the stored effective_date, and src/ops/cancel_subscription.rs stores that date as the end of the calendar month (cause 1), so inside any single session the record is never due and never removed; the replay reports "error code None not in ['ResourceNotFoundException'] after 15 attempt(s)".
Why: this breaks the only observable end of the subscription's life. Teardown code that cancels and then waits for the resource to disappear hangs or times out; anything that counts ACTIVE subscriptions keeps counting a cancelled one (and the emulator's own MAX_SUBSCRIPTIONS capacity, src/state.rs:38, is consumed by it); and dependent gates keyed on "a non-terminated subscription covers this resource" — the CloudFront web-ACL lock, the resource-already-subscribed refusal — stay armed forever, so a correct teardown order is rejected.
Fix: CancelSubscription's handler and the scheduled-change processing in crate::state (src/ops/cancel_subscription.rs plus state.rs apply_due_changes) — complete the cancellation for a subscription that has no billing commitment (FREE tier, and PENDING_APPROVAL per the operation's documentation) by removing the record rather than parking it on a month boundary, so GetSubscription answers the modeled ResourceNotFoundException with the message above and ListSubscriptions stops reporting it.
Traces: pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle#task#11 (cluster of 3 steps across 3 fixtures).

## Evidence
Replay of trace set simparity-wire-aws-v1. Instants, ARNs and ids below are per-run values filling the templates.

Cause 1 — fixture pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle step task#10 (CancelSubscription, expected 200, got 200; cluster of 3 steps):
  $.scheduledChange.effectiveDate: type_mismatch — expected the member to be ABSENT (None), got 1796083200 (the first instant of the next calendar month).
  Smithy: ScheduledChange.effectiveDate optional Timestamp, doc "This value is populated after the change is confirmed by the billing system."
  Source: src/state.rs:501-521 (end_of_billing_period), :154 (ScheduledChangeRecord.effective_date), :163-170 (to_shape publishes it).
  (The same step also reports createdAt/updatedAt as epoch numbers — that is the serializer cause filed under the subscription-representation issue, not restated here.)

Cause 2 — same fixture step task#11 (GetSubscription, expected 404, got 200; cluster of 3 steps):
  status: expected 404, got 200
  $.message: missing — expected "Subscription not found: <subscription arn>. Verify the ARN is correct and the subscription exists in your account.", got None
  $headers.x-amzn-errortype: missing — expected the ResourceNotFoundException error type, got None
  $: poll: error code None not in ['ResourceNotFoundException'] after 15 attempt(s); the emulator kept returning {'planFamily': 'CloudFront', 'planTier': 'FREE', 'status': 'ACTIVE', 'scheduledChange': {'changeType': 'CANCELLATION', 'effectiveDate': <next month>}, ...}
  Source: src/state.rs:409-451 apply_due_changes (removal gated on the stored effective_date having passed).

## Steps to Reproduce
1. Point a client at the emulator (region us-east-1, pricing-plan-manager endpoint).
2. CreateSubscription(planFamily="CloudFront", planTier="FREE", resourceArns=[<CloudFront distribution ARN>, <its CLOUDFRONT-scope web ACL ARN>]); poll GetSubscription until status=="ACTIVE"; keep the current eTag.
3. CancelSubscription(arn=<sub arn>, ifMatch=<current eTag>).
4. Observe the 200 body: scheduledChange.changeType=="CANCELLATION" (correct) but scheduledChange.effectiveDate is already populated with the start of next month. AWS omits effectiveDate until the billing system confirms the change.
5. Poll GetSubscription(arn=<sub arn>) repeatedly (the fixture polls 15 times). The emulator keeps answering HTTP 200 with status "ACTIVE"; AWS answers HTTP 404 ResourceNotFoundException "Subscription not found: <arn>. Verify the ARN is correct and the subscription exists in your account." once the cancellation completes.
6. ListSubscriptions() also still reports the cancelled subscription.

Replay: replay_wire_corpus and re-run fixture pricing-plan-manager-cloudfront-flat-rate-subscription-lifecycle steps task#10 and task#11 (the task#11 cluster also covers pricing-plan-manager-free-subscription-hosted-zone-association#task#15 and pricing-plan-manager-free-subscription-token-replay-and-etag-rules#task#15).
