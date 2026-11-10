Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: b35089c281a7e84768d928c8c72034d27eee1ae8

# The allocation-expiry boundary rule is computed twice: a credit code re-implements has_expired instead of reaching it

_Filed: 2026-11-10T21:04:56Z_
_Operation: GetBenefitAllocation | Class: maintainability_

> _Note: this issue was identified against the code base at commit `b35089c281a7e84768d928c8c72034d27eee1ae8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate states the expiry boundary ONCE as a named rule — `BenefitAllocation::has_expired`, documented as "`ExpiresAt` is the instant the model documents it 'expires and is no longer usable' at, so the term is closed from that instant on" — and `BenefitAllocation::expire_due` reaches it. The credit-code path does NOT: `AllocatedCreditCode::expire_due` re-writes the same boundary comparison inline against its own `expires_at_millis` (`now_millis < self.expires_at_millis`, i.e. expired iff `now_millis >= self.expires_at_millis`), even though its own doc claims it applies the rule "exactly as [`BenefitAllocation::expire_due`] does for the allocation".

So the one rule that decides whether a term has closed at an instant exists as two copies: the named accessor and a hand-written comparison. The two agree today (both are `>=` on the same field name), which is why this has no behavioural impact — it is purely that the copies can drift: changing the accessor to an exclusive comparison, or adding a grace window to it, would silently leave the credit-code copy on the old boundary, and no existing test can see the difference because `place_expiry` sets a code's `expires_at_millis` to the same instant as its allocation's (`tests/all/get_benefit_allocation.rs:704-717`).

The fix is to compute the boundary in one place and have both call it (e.g. a free `fn term_closed(expires_at_millis: i64, now_millis: i64) -> bool` that `has_expired` and `AllocatedCreditCode::expire_due` both reach), leaving every response and every existing test unchanged.

## Evidence

The named rule — `services/.../src/state.rs:1006-1011`:
```
    /// Whether this allocation's term has closed at `now_millis` — `ExpiresAt` is the
    /// instant the model documents it "expires and is no longer usable" at, so the term is
    /// closed from that instant on.
    pub fn has_expired(&self, now_millis: i64) -> bool {
        now_millis >= self.expires_at_millis
    }
```
and its one caller — `services/.../src/state.rs:1023-1029`:
```
    pub fn expire_due(&mut self, now_millis: i64) -> bool {
        let mut changed = false;
        if self.status == ALLOCATION_STATUS_ACTIVE && self.has_expired(now_millis) {
```

The second copy — `services/.../src/state.rs:845-852`:
```
    /// Materialise the expiry the clock has already made due on this code, exactly as
    /// [`BenefitAllocation::expire_due`] does for the allocation: a code past its own
    /// `ExpiresAt` can no longer be redeemed, so it is `INACTIVE` whether or not anybody
    /// has looked at it. A redeemed (`FULFILLED`) code is terminal.
    fn expire_due(&mut self, now_millis: i64) -> bool {
        if self.status != ALLOCATION_STATUS_ACTIVE || now_millis < self.expires_at_millis {
            return false;
        }
```
The doc says "exactly as [`BenefitAllocation::expire_due`] does", but the comparison is written out again rather than reaching the accessor that `BenefitAllocation::expire_due` uses — and `state.rs:1020-1022` repeats the claim from the other side: "Each credit code the allocation has issued expires on the same rule, against its own `ExpiresAt`".

Test coverage that must stay green and unchanged:
- `tests/all/get_benefit_allocation.rs:607-689` `a_read_materialises_a_due_expiry_and_changes_nothing_else` — pins both halves: "an allocation past its ExpiresAt is never ACTIVE" (line 629-632) and "a code past its ExpiresAt can no longer be redeemed" (line 684-688).
- `tests/all/benefit_allocations.rs:811-869` (rule 5) — pins the allocation half over the listing, selecting the due rows through the accessor itself: `.filter(|allocation| allocation.has_expired(now))` (line 823).

Reached from the wire by this operation: `src/ops/get_benefit_allocation.rs:274` `store.expire_allocations_due(&input.catalog, now_millis);` → `state.rs:2041-2047` → `BenefitAllocation::expire_due` → both copies.

## Steps to Reproduce

1. Read `services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/src/state.rs:1006-1011` — the expiry boundary as the one named rule, `has_expired`, with the doc that states it is closed from `ExpiresAt` on.
2. Read `state.rs:1023-1029` — `BenefitAllocation::expire_due` applies that rule by calling the accessor.
3. Read `state.rs:845-852` — `AllocatedCreditCode::expire_due` claims to apply the rule "exactly as [`BenefitAllocation::expire_due`] does" but spells the comparison out again (`now_millis < self.expires_at_millis`) instead of reaching one shared boundary helper.
4. Confirm the drift is invisible to the suite: `tests/all/get_benefit_allocation.rs:704-717` (`place_expiry`) gives a code the same `expires_at_millis` as its allocation, so no test exercises the two boundaries at different instants.
5. Fix by computing the boundary once (one helper both reach); responses and both pinned tests stay byte-identical.
