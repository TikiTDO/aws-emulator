Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__pricing-plan-manager__route53__wafv2.free-subscription-refuses-a-key-value-store-and-keeps-its-resources cloudfront__pricing-plan-manager__route53__wafv2.hosted-zone-joins-and-leaves-a-free-subscription
Issue-Key: cloudfront__pricing-plan-manager__route53__wafv2.free-subscription-refuses-a-key-value-store-and-keeps-its-resources|run#12|AssertionError|assert (code, message) == ("ValidationException", (
Issue-Key: pricing-plan-manager|AssociateResourcesToSubscription|NotImplemented
Issue-Commit: 14df30a096a54aecb31fd12ed18f88f92038b3f7

# Implement pricing-plan-manager AssociateResourcesToSubscription: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `14df30a096a54aecb31fd12ed18f88f92038b3f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateResourcesToSubscription

## Description
`AssociateResourcesToSubscription` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-pricingplanmanager-awspricingplanmanager/src/ops/associate_resources_to_subscription.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `AssociateResourcesToSubscription` call, so each one unblocks only when `AssociateResourcesToSubscription` works: `cloudfront__pricing-plan-manager__route53__wafv2.free-subscription-refuses-a-key-value-store-and-keeps-its-resources`, `cloudfront__pricing-plan-manager__route53__wafv2.hosted-zone-joins-and-leaves-a-free-subscription`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-pricingplanmanager-awspricingplanmanager/src/ops/associate_resources_to_subscription.rs`. Replaying the corpus, the emulator refused the `AssociateResourcesToSubscription` call: File "run.py", line 365, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_cloudfront__pricing_plan_manager__route53__wafv2_free_subscription_refuses_a_key_value_store_and_keeps_its_resources.py", line 320, in run
    assert (code, message) == ("ValidationException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: unexpected answer to a KeyValueStore on the FREE tier: NotImplemented: 'operation AssociateResourcesToSubscription is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the pricing-plan-manager emulator
Call AssociateResourcesToSubscription and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
