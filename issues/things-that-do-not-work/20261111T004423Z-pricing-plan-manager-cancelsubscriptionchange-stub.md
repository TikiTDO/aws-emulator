Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__pricing-plan-manager__wafv2.cancel-subscription-change-without-pending-change-rejected
Issue-Key: cloudfront__pricing-plan-manager__wafv2.cancel-subscription-change-without-pending-change-rejected|run#9|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}: {message!r}"
Issue-Commit: c6516c5463abfac6c10762734949ce363897af5e

# Implement pricing-plan-manager CancelSubscriptionChange: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `c6516c5463abfac6c10762734949ce363897af5e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelSubscriptionChange

## Description
`CancelSubscriptionChange` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-pricingplanmanager-awspricingplanmanager/src/ops/cancel_subscription_change.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CancelSubscriptionChange` call, so each one unblocks only when `CancelSubscriptionChange` works: `cloudfront__pricing-plan-manager__wafv2.cancel-subscription-change-without-pending-change-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-pricingplanmanager-awspricingplanmanager/src/ops/cancel_subscription_change.rs`. Replaying the corpus, the emulator refused the `CancelSubscriptionChange` call: File "run.py", line 365, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_cloudfront__pricing_plan_manager__wafv2_cancel_subscription_change_without_pending_change_rejected.py", line 267, in run
    assert code == "ValidationException", f"expected ValidationException, got {code}: {message!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ValidationException, got NotImplemented: 'operation CancelSubscriptionChange is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the pricing-plan-manager emulator
Call CancelSubscriptionChange and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
