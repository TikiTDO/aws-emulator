Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 519464c562749a972ee96f937e64e09278d4df74

# GetCreditAllocationHistory never traverses the consolidated billing family, on the false premise that this crate persists none

_Filed: 2026-11-01T12:52:00Z_
_Operation: GetCreditAllocationHistory | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `519464c562749a972ee96f937e64e09278d4df74`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model says this operation reports cross-account credit applications by walking the consolidated billing family of the account asked about. The handler instead selects allocations whose stored `account_id` is EXACTLY the requested `accountId`, and justifies that with a comment asserting that the emulator keeps no family at all — which is untrue of this very crate: `ComAmazonawsBillingAwsbillingState` persists `consolidated_billing_family`, exposes `family_account_name`, and the sibling read `GetCredits` widens its selection to the whole family for the payer account. So an allocation applied to a FAMILY MEMBER's bill is invisible to a `GetCreditAllocationHistory` call made for the family's management (payer) account, and the stale comment will keep the gap invisible to the next reader: it says a family model "would widen this filter", as though the family did not already exist in the store beside it.

## Evidence

Smithy (query="doc", shape="GetCreditAllocationHistory"): "Returns the per-billing-month allocation history for credits applied to an Amazon Web Services account's bills. **Traverses the consolidated billing family to capture cross-account credit applications.** Supports pagination and optional filtering to a single credit."

Handler — services/service-com-amazonaws-billing-awsbilling/src/ops/get_credit_allocation_history.rs:264-267:
```
    // This emulator persists no consolidated billing family, so the family of
    // the requested account is that account: the entries applied to its own
    // bills. A family model would widen this filter, not replace it.
    allocation.account_id == input.account_id
```

The same crate DOES persist that family — services/service-com-amazonaws-billing-awsbilling/src/state.rs:530-532:
```
    /// The consolidated billing family: account id -> that account's display
    /// name, which `accountName` reports beside `accountId`.
    pub consolidated_billing_family: BTreeMap<String, String>,
```
and state.rs:731-737 `pub fn family_account_name(&self, account_id: &str) -> Option<&str>` — "The display name of a member of the consolidated billing family".

The sibling credit read already widens to the family — state.rs:889-895 (`credits_for_period`): "`whole_family` widens the selection to every account of the consolidated billing family — every account this store knows, since the emulator's own account (`engine::emulator_account_id`) is the payer account of its own consolidated billing family." — called from src/ops/get_credits.rs:221-225:
```
        let whole_family = input.payer_account_flag.unwrap_or(false)
            && input.account_id == ::engine::emulator_account_id();
        let credits = state.credits_for_period(&input.account_id, whole_family, start_secs, end_secs)
```

Test coverage: tests/all/credit_allocation_history.rs:53-55 declares `OTHER_ACCOUNT_ID = "123456789012"` — "A second 12-digit account, used to pin that the history of one account never leaks into another's" — and line 233 seeds "Another account's allocation: never reported for ACCOUNT_ID." The model sentence above contradicts that stated reason for the payer case. Note that `OTHER_ACCOUNT_ID` is NOT a member of `consolidated_billing_family` (state.rs:604-608 seeds only the caller account), so a fix that traverses actual family MEMBERSHIP leaves that assertion — and every other row of that suite, all seeded for `ACCOUNT_ID` — passing.

## Steps to Reproduce

1. Boot the emulator and add a second account to the billing family (state.rs:532 `consolidated_billing_family`), e.g. account 210987654321, as `GetCredits`' own suite does for the family case.
2. Record a credit allocation whose `account_id` is that family member (state.rs:681 `record_credit_allocation`).
3. Call GetCreditAllocationHistory with `accountId` = the family management account (000000000000) and a `startDate`/`endDate` range covering that allocation's `billingMonth`.
4. Observe `creditAllocationHistoryList` is empty: the family member's allocation is filtered out by `allocation.account_id == input.account_id` (get_credit_allocation_history.rs:267), although the operation's model doc says it "Traverses the consolidated billing family to capture cross-account credit applications".
5. For contrast, call GetCredits with the same `accountId` and `payerAccountFlag: true` and observe the family member's credit IS reported (get_credits.rs:221-225).
