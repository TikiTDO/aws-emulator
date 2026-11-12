Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c068b04c9070a7dffb3e8c7bd93f77709d5e2d9a

# Account memberships that cover no billing period fill the per-account history cap, so a real past membership is evicted

_Filed: 2026-11-12T06:04:57Z_
_Operation: AssociateAccounts | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c068b04c9070a7dffb3e8c7bd93f77709d5e2d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A billing-group membership that begins and ends in the SAME billing period is in force in no period at all, yet it is kept as a stored record and counts against the per-account history cap — and the cap loop in `associate_account` evicts `memberships.remove(0)`, the OLDEST record, which is the membership an earlier (closed) billing period must still report.

How a caller reaches it: `DisassociateAccounts` ends the live membership at the current billing period in place (`release_account`), and a following `AssociateAccounts` in that SAME period finds no live record and APPENDS a new one. Each associate/disassociate cycle inside one month therefore leaves one more record whose inclusive start and exclusive end are the same period — a record `AccountAssociationRecord::covers` reports for no period at all. After MAX_ACCOUNT_ASSOCIATIONS_PER_ACCOUNT (100) such cycles the vector is full of records no read can report, and the next `associate_account` drops index 0: the account's genuine membership of an earlier month. From then on `ListAccountAssociations(BillingPeriod=<that earlier month>)` reports the account as belonging to no billing group (UNMONITORED), and `billing_group_sizes_in` — the one rule `ListBillingGroups` and `UpdateBillingGroup` report `Size` from — counts the group one member short for that month.

The crate already states the opposite rule for the structurally identical record on the sibling resource: a pricing-rule association ended in the very period it started in is DROPPED rather than kept as a span covering nothing, explicitly so that a caller cycling one rule inside one billing period cannot grow the record, and only ENDED associations are counted against that store's cap. The account-membership path does neither, which also contradicts the rationale its own constant carries.

## Evidence

The eviction, blind to whether the evicted record reports anything — `src/state.rs:3141-3149`:
```rust
        memberships.push(AccountAssociationRecord {
            billing_group_arn: group_arn.to_string(),
            inclusive_start_billing_period: billing_period,
            exclusive_end_billing_period: None,
        });
        while memberships.len() > MAX_ACCOUNT_ASSOCIATIONS_PER_ACCOUNT {
            memberships.remove(0);
        }
```

The record that covers nothing is created and kept — `src/state.rs:3163-3164` (`release_account`): `if live.billing_group_arn == group_arn { live.exclusive_end_billing_period = Some(billing_period); }`, and `src/state.rs:2082-2084` says what such a record reports: "Was the account a member of this group in `billing_period`? Half-open, as [`billing_period_range_covers`] reads it, so a membership that began and ended in the SAME period covers no period at all".

The cap's own stated rationale, which the dead records break — `src/state.rs:4336-4341`: "A CHOSEN bound, not an AWS limit: a caller decides how often it associates and disassociates an account, and each cycle is one more ended membership to report a past billing period from. The oldest ENDED membership is dropped first, so a forgotten one reads back the way that period read before anybody recorded it, and the membership in force is never the one evicted. A billing period is a month, so this holds every membership a long history of them could be reported for."

The sibling path that does it the other way — `src/state.rs:1194-1209`: "An association ended in the very period it started in was in force in no period at all ([`PricingRuleAssociationRecord::covers`]), so its record is DROPPED rather than kept as a span covering nothing — which is also what keeps a caller cycling one rule inside one billing period from growing the record. The ended associations that a read of a closed period still has to report are kept, at most [`MAX_ENDED_PRICING_RULE_ASSOCIATIONS_PER_PRICING_PLAN`] of them with the oldest dropped first" over
```rust
        self.pricing_rule_associations
            .retain(PricingRuleAssociationRecord::covers_any_period);
        let cap = MAX_ENDED_PRICING_RULE_ASSOCIATIONS_PER_PRICING_PLAN;
        while self.ended_rule_association_count() > cap {
```

The reader that loses the evicted period — `src/ops/list_account_associations.rs:228` (`state.linked_accounts(&billing_period, ...)`) and `src/state.rs:3227-3245` (`billing_group_sizes_in`, both answering through `membership_in`).

Test coverage: no test in `tests/all/` exercises the cap (`grep -n 'MAX_ACCOUNT_ASSOCIATIONS' tests/` matches nothing); `tests/all/account_association_billing_periods.rs` and `tests/all/membership_writes_keep_superseded_generations.rs` pin that an earlier period keeps reporting the membership it held, which is exactly what the eviction drops.

## Steps to Reproduce

1. Make the emulated account the payer so the control plane is reachable (the store's `payer_account_id`), then CreatePricingPlan + CreateBillingGroup G0 and AssociateAccounts(G0, account X) — in month M1 this records the membership (G0, M1, open).
2. In a later month M2, DisassociateAccounts(G0, [X]) then AssociateAccounts(G0, [X]) — the first call ends the live membership at M2, the second appends a fresh (G0, M2, open) record, leaving (G0, M2, M2) behind, a record `AccountAssociationRecord::covers` answers `false` for in every period.
3. Repeat that pair 100 times inside month M2.
4. ListAccountAssociations with BillingPeriod=M1: the account is reported with no `BillingGroupArn` (UNMONITORED) instead of G0, because the oldest record — the M1 membership — was evicted by `memberships.remove(0)` to make room for records that report no period at all.
5. ListBillingGroups with BillingPeriod=M1 (or UpdateBillingGroup's `Size`): G0's `Size` is one smaller than the membership that month really held.
