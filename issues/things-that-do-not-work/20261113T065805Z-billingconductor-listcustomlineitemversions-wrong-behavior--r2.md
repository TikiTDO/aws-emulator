Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a30cb6a6b6905f387bbecd05254c1537fe1d9d44

# The billing-period filter matches a bisected generation on its OUTER bounds, so a version is reported for a range it applies in no period of

_Filed: 2026-11-13T06:58:05Z_
_Operation: ListCustomLineItemVersions | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a30cb6a6b6905f387bbecd05254c1537fe1d9d44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListCustomLineItemVersions` selects versions with `in_filter`, which is handed only TWO bounds per generation: `version.start_billing_period()` (the FIRST stretch's inclusive start) and `end_billing_period(...)` (the LAST stretch's exclusive end). For a generation that applies over a SINGLE stretch of billing periods those two bounds are the stretch, and the selection is right. For a generation carved into two stretches they are the HULL across the hole, so the filter matches on billing periods the generation applies in none of.

A bisected generation is not hypothetical: a bounded `DeleteCustomLineItem` whose range falls inside the stretch the item already applies over splits that stretch in two (`remove_billing_periods` / `BillingPeriodSpan::minus`), and the crate's own suite builds exactly such an item (`tests/all/delete_custom_line_item.rs:1110-1120`, "the history of a bisected item").

The result is that two readers of the same stored item contradict each other for the same billing period. For a period inside the hole:
- `ListCustomLineItems` reports NOTHING, because `CustomLineItemRecord::effective_in` asks each STRETCH whether it covers the period (`CustomLineItemVersion::covers`);
- `ListResourcesAssociatedToCustomLineItem` reports nothing either, because the bounded delete narrowed every association by the same periods;
- `ListCustomLineItemVersions`, filtered to exactly that period, reports the generation as a version "applied" there.

The handler's own `in_filter` doc states the rule it fails to carry out for a bisected generation ("A generation is selected when its own half-open span shares at least one billing period with that span"), and the per-stretch answer it should ask for is the one the crate already owns and uses everywhere else a period is asked about a generation: `CustomLineItemVersion::covers`. The hull is correct for the REPORTED `StartBillingPeriod`/`EndBillingPeriod` pair — `src/state.rs:2500-2506` says so explicitly, and a test pins it — but that same doc says which periods inside the hull the item is really reported for "is decided by the stretches themselves", which is the decision the filter skips.

This is a third, distinct case from the two already-fixed defects on this operation: `...-listcustomlineitemversions-validation.md` was the filter's end bound being read as inclusive, and `...-listcustomlineitemversions-wrong-behavior.md` was a generation applying in NO billing period at all being reported. Here the generation does apply somewhere, and the filter's bounds are read correctly — it is the multi-stretch generation whose periods are matched by hull rather than per stretch.

## Evidence

The selection, which reduces a generation to two bounds before testing the filter — `src/ops/list_custom_line_item_versions.rs:334-342`:

        for (index, version) in versions.iter().enumerate() {
            let end = end_billing_period(version, versions.get(index + 1));
            if !in_filter(&version.start_billing_period(), end.as_deref(), filter) {
                continue;
            }
            // The pagination key is the generation's position in that order, written
            // fixed-width so it orders as a string the way it orders as a number.
            selected.push((format!("{index:010}"), element(item, version, end)));
        }

The rule that selection states about itself — `src/ops/list_custom_line_item_versions.rs:148-160`:

    /// Is a generation in effect anywhere inside the billing periods the request's filter
    /// names?
    ...
    /// A generation is selected when its own half-open span shares at least one billing
    /// period with that span, which is the crate's one half-open overlap decision
    /// ([`crate::state::half_open_ranges_overlap`]) on the billing-period axis.

but the two bounds it is given are the OUTER ones, and the same doc says the periods inside them are decided elsewhere — `src/state.rs:2487-2511`:

    /// `StartBillingPeriod`: the billing period this generation is in effect FROM,
    /// inclusive.
    ...
    pub fn start_billing_period(&self) -> String {
        self.spans
            .first()
            .and_then(|span| span.inclusive_start.clone())
            .unwrap_or_else(|| billing_period_at(self.start_time_millis))
    }

    /// The billing period this generation stops applying at, EXCLUSIVE; absent when it
    /// does not stop.
    ///
    /// A `CustomLineItemVersionListElement` carries ONE start and end billing period, so
    /// a generation is reported by the outer bounds of the stretches it applies in;
    /// which periods inside them a read reports the item for is decided by the stretches
    /// themselves ([`Self::covers`]).
    pub fn exclusive_end_billing_period(&self) -> Option<String> {
        self.spans
            .last()
            .and_then(|span| span.exclusive_end.clone())
    }

The per-stretch decision every OTHER period-scoped read of the same generation uses — `src/state.rs:2473-2475` and `src/state.rs:2257-2266`:

    pub fn covers(&self, billing_period: &str) -> bool {
        self.spans.iter().any(|span| span.covers(billing_period))
    }
    ...
    pub fn effective_in(&self, billing_period: &str) -> Option<CustomLineItemVersion> {
        let current = self.current_version();
        if current.covers(billing_period) {
            return Some(current);
        }

The model's own words for the filter the handler is applying (`smithy_query(query="shape",shape="ListCustomLineItemVersionsBillingPeriodRangeFilter")`):

    StartBillingPeriod: "The inclusive start billing period that defines a billing period
    range where a custom line item version is applied."
    EndBillingPeriod: "The exclusive end billing period that defines a billing period
    range where a custom line item version is applied."

That a generation really is carved into two stretches by a bounded delete — `src/state.rs:4411-4418`:

    /// A CHOSEN bound, not an AWS limit: a caller decides how many bounded
    /// `BatchDisassociateResourcesFromCustomLineItem` or `DeleteCustomLineItem` calls to
    /// make, and one whose range falls inside a stretch the resource already has splits it
    /// in two

Test coverage: the bisected item exists in the suite, but only the UNFILTERED listing is driven over it — `tests/all/delete_custom_line_item.rs:1105-1127`:

    // What is left of the item is still ONE stored item with its own history: a
    // `CustomLineItemVersionListElement` carries one start and one end billing period,
    // so the generation is reported by the OUTER bounds of the stretches it still
    // applies in.
    ...
    emulator
        .delete_ok(case, &bounded, &range(&period(0), Some(&period(2))))
        .await;
    assert!(
        emulator.reports(case, &bounded, Some(&period(5))).await
            && !emulator.reports(case, &bounded, Some(&period(1))).await,

(that assertion is the contradiction: `period(1)` is inside the hole and the item is NOT reported there). The filter's own test states the rule but drives only single-stretch generations — `tests/all/list_custom_line_item_versions.rs:938-940` and `:1018-1029`:

    /// THE RULE on the filter: a billing-period range selects the generations in effect
    /// anywhere inside it — its start bound INCLUSIVE and its end bound EXCLUSIVE, as the
    /// model documents them, and an absent bound open.
    ...
        for (start, end) in [(period(-1), None), (period(0), Some(period(4)))] {
            emulator
                .update_ok(
                    case,
                    &item,
                    json!({ "BillingPeriodRange": period_range(&start, end.as_deref()) }),
                )

## Steps to Reproduce

1. Start the emulator with the emulated account designated the payer account (as every suite does: `engine.state::<BillingConductorState>().lock().payer_account_id = Some(emulator_account_id())`). Call the current billing period M, and M+n / M-1 the months around it.
2. POST /create-pricing-plan, then POST /create-billing-group -> group G.
3. POST /create-custom-line-item against G with `{"ChargeDetails":{"Type":"FEE","Flat":{"ChargeValue":100}},"BillingPeriodRange":{"InclusiveStartBillingPeriod":"M-1","ExclusiveEndBillingPeriod":"M+6"}}` -> item I, one generation applying over the single stretch [M-1, M+6).
4. POST /delete-custom-line-item with `{"Arn": I, "BillingPeriodRange":{"InclusiveStartBillingPeriod":"M","ExclusiveEndBillingPeriod":"M+2"}}` (accepted: the start is the current billing period). The generation is now carved into the two stretches [M-1, M) and [M+2, M+6) — the "bisected item" of tests/all/delete_custom_line_item.rs:1110.
5. POST /list-custom-line-items with `{"BillingPeriod":"M+1"}` -> I is NOT reported (M+1 is in the hole; `effective_in` asks each stretch).
6. POST /list-custom-line-item-versions with `{"Arn": I, "Filters":{"BillingPeriodRange":{"StartBillingPeriod":"M","EndBillingPeriod":"M+2"}}}`.
7. Observed: HTTP 200 with ONE version (StartBillingPeriod M-1, EndBillingPeriod M+6) — the generation is selected because the hull [M-1, M+6) overlaps [M, M+2), although it applies in neither M nor M+1.
8. Expected, per the filter's modelled words ("a billing period range where a custom line item version is applied") and the crate's own per-stretch decision `CustomLineItemVersion::covers` (src/state.rs:2473-2475), which the sibling period-scoped reads use: no version at all, exactly as /list-custom-line-items reports nothing for M and M+1.
9. Contrast inside one range: the same request with `{"StartBillingPeriod":"M+2","EndBillingPeriod":"M+3"}` correctly reports the version, and with `{"StartBillingPeriod":"M-1","EndBillingPeriod":"M"}` too — only the hole is answered wrongly.
