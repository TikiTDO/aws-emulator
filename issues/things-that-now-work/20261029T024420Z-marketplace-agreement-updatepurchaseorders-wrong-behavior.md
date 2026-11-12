Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: cd4f62d4622d99c848b6778d8f7c3e7dcd5cf5b1

# UpdatePurchaseOrders stamps updated_at without materialising a due expiry, so the next reader rewinds lastUpdateTime to before the association

_Filed: 2026-10-29T02:44:20Z_
_Operation: UpdatePurchaseOrders | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `cd4f62d4622d99c848b6778d8f7c3e7dcd5cf5b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdatePurchaseOrders` is the only mutating handler in this crate that writes an agreement record without first calling `crate::state::expire_agreement`, the crate's single materialiser of an expiry the agreement's own stored `endTime` has already made due. Every sibling that touches one agreement calls it first (`CancelAgreement`, `DescribeAgreement`, `GetAgreementEntitlements`, `SendAgreementCancellationRequest`, `SendAgreementPaymentRequest`, `AcceptAgreementCancellationRequest`, and `AcceptAgreementRequest` for its source).

Consequence: for an agreement whose `endTime` has already passed but whose expiry nobody has materialised yet, `UpdatePurchaseOrders` records the association and stamps `agreement.updated_at = now` (an instant AFTER the end date). The next reader then calls `expire_agreement`, which writes `updated_at = end` — an EARLIER instant. So `AgreementViewSummary$lastUpdateTime` moves BACKWARDS across two reads, the association this caller made stops being the newest change the record reports, and because `updated_at` is also the key `SearchAgreements` sorts by (`sortBy=LastUpdateTime`) and the value its `BeforeLastUpdateTime`/`AfterLastUpdateTime` filters select on, the agreement silently moves in the page order and can drop out of a window that bounded it a moment earlier. The purchase-order association itself survives; the record of when it happened does not.

The same write path is reached from `AcceptAgreementPaymentRequest` (`approve_payment_request` -> `associate_purchase_order`), which also skips `expire_agreement`; both are symptoms of the one missing call and belong to this one finding.

## Evidence

The crate's own rule for what `updated_at` means after an expiry — services/.../src/state.rs:2266-2267:
  "/// `updated_at` becomes the instant the change was DUE, never the instant it
   /// was observed, so repeated reads report the same `lastUpdateTime`."
and the write that implements it, services/.../src/state.rs:2292:
  "let _ = self.leave_active(STATUS_EXPIRED, end);"
(`leave_active` sets `self.updated_at = now` with `now = end`, state.rs:2211-2212).

The conflicting write, services/.../src/state.rs:2180-2192 (`Agreement::associate_purchase_order`):
  "/// The agreement's own record changed, which is what
   /// `AgreementViewSummary$lastUpdateTime` reports.
   pub fn associate_purchase_order(&mut self, association: PurchaseOrder, now: SystemTime) {
   ...
       self.updated_at = now;"

The handler that reaches it with no expiry materialised — services/.../src/ops/update_purchase_orders.rs:163-164 and :261:
  "let now = ::std::time::SystemTime::now();
   let mut store = state.lock();"
  "agreement.associate_purchase_order(association, now);"
(no `store.expire_agreement(...)` anywhere in the file).

The sibling that does it, services/.../src/ops/cancel_agreement.rs:151-154:
  "// The guard below acts on the status the clock has already left the
   // agreement in: one past its own stored end date expired at that
   // instant ...
   store.expire_agreement(&input.agreement_id, now);"
and services/.../src/ops/describe_agreement.rs:195: "store.expire_agreement(&input.agreement_id, now);".

What consumes the rewound value — services/.../src/state.rs:2527 (`sort_key`): "\"LastUpdateTime\" => Some(self.updated_at),"; services/.../src/state.rs:3237 (`matches_filter`): "\"BeforeLastUpdateTime\" => before(Some(agreement.updated_at)),"; services/.../src/state.rs:2466 (`to_view_summary`): "last_update_time: Some(::aws_smithy_types::DateTime::from(self.updated_at)),".

The sibling write with the same omission — services/.../src/ops/accept_agreement_payment_request.rs:268-272 (`store.approve_payment_request(...)`, no `expire_agreement`) reaching state.rs:4716 (`agreement.associate_purchase_order(..., now)`).

Test coverage: tests/all/agreement_expiry.rs winds a stored agreement's own window back (`signed_ago`, see its header at lines 38-41: "The clock cannot be wound forward 30 days inside a test, so the agreement's own stored window is wound BACK instead") and pins the EXPIRED materialisation for the readers; tests/all/purchase_order_updates.rs and tests/all/purchase_order_associations.rs cover the association on an unexpired agreement. No test combines the two, which is why the rewind is unpinned.

## Steps to Reproduce

1. Create and accept an agreement request whose terms commit a contract length (e.g. a ConfigurableUpfrontPricingTerm with selectorValue P1M), so the agreement gets a stored endTime, and note its agreementId and one chargeId from ListAgreementCharges.
2. Age the stored record past its end date the way tests/all/agreement_expiry.rs does (wind the agreement's start/acceptance/end instants back by more than the contract length), without calling any operation that materialises expiry.
3. Call UpdatePurchaseOrders with purchaseOrders=[{chargeId, chargeRevision: 1, purchaseOrderReference: "PO-1"}] — it answers 200 and stamps agreement.updated_at = now (after the end date).
4. Call SearchAgreements (filters: AgreementType=PurchaseAgreement) and read agreementViewSummaries[0].lastUpdateTime: it reports the agreement's END date, i.e. an instant EARLIER than the association made in step 3 — the expire() write that ran during this call overwrote it.
5. Repeat SearchAgreements with filter AfterLastUpdateTime set just before the step-3 instant: the agreement is not returned, although its newest change happened after that bound.
