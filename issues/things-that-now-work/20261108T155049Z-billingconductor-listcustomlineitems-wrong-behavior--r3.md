Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0a1e14ce9c5d04d4281ea317c037d5ece999a2b8

# A superseded custom-line-item generation keeps a stale COPY of the item's resource associations, so AssociationSize contradicts the two other readers of the same associations

_Filed: 2026-11-08T15:50:49Z_
_Operation: ListCustomLineItems | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0a1e14ce9c5d04d4281ea317c037d5ece999a2b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CustomLineItemVersion` stores its own `Vec<ResourceAssociationRecord>`, frozen by `put_custom_line_item` at the moment the generation is superseded. Nothing in the crate ever maintains that copy again: `BatchAssociateResourcesToCustomLineItem`, `BatchDisassociateResourcesFromCustomLineItem`, `remove_custom_line_item` (and therefore `DeleteCustomLineItem` / `DeleteBillingGroup`) and `remove_custom_line_item_periods` all write ONLY `CustomLineItemRecord::associated_resources`, the live list.

The result is that `AssociationSize`, which `ListCustomLineItems` and `ListCustomLineItemVersions` take from `version.association_size(billing_period)`, is computed from the stale snapshot whenever the period being read is answered by a superseded generation, while the two other readers of the very same associations — `ListResourcesAssociatedToCustomLineItem` and the pro forma charge computation `charge_input` behind `GetBillingGroupCostReport` / `ListBillingGroupCostReports` — are computed from the live records. For one billing period the two answers disagree: `AssociationSize` can count a resource that was disassociated from that period, or one whose record no longer exists at all, which the listing of the associations reports as absent and the cost report prices as absent.

The crate states the opposite rule in the very function that does the counting (`associations_in_effect`: "Counted here once, for the live item ... as much as for a frozen generation of it, so no two reads of one period can disagree about the number"), and both sibling versioned resources deliberately do NOT freeze their membership into a generation: `PricingPlanVersion`'s doc says `Size` "is derived per period from the plan's associations ... rather than frozen into a generation", and `BillingGroupVersion`'s says "The membership a period reports is not here either: it is kept per period on the account memberships themselves". A resource association already carries the billing periods it is in effect over, so the live records alone answer "the membership the item really had then" — the frozen copy adds nothing but the drift.

## Evidence

The frozen copy and the claim it makes — `src/state.rs:2403-2407`:
```
    /// The resource associations this generation held, as they stood while it was in
    /// effect, so the `AssociationSize` a read reports for it ([`Self::association_size`])
    /// is the membership the item really had then and not a recomputation from the
    /// associations it has since acquired.
    pub associated_resources: Vec<ResourceAssociationRecord>,
```
taken once, at `src/state.rs:2292` inside `CustomLineItemRecord::current_version()`:
```
            associated_resources: self.associated_resources.clone(),
```
and frozen by `src/state.rs:3509-3532` (`put_custom_line_item`), which clones `previous.current_version()` and pushes it into `superseded_versions` — after which no code path writes to it (`grep -n superseded_versions src` shows no mutation of a version's `associated_resources`).

The rule the crate states for exactly this count — `src/state.rs:2000-2007`:
```
/// Every read reporting that member is scoped to ONE billing period, and whether a
/// resource is associated IN a period is the association record's own question
/// ([`ResourceAssociationRecord::covers`]) — the same one the listing of the associations
/// and the charge computation ask of it — so a record the item holds for periods that do
/// not include the one being read counts for nothing there. Counted here once, for the
/// live item ([`CustomLineItemRecord::association_size`]) as much as for a frozen
/// generation of it ([`CustomLineItemVersion::association_size`]), so no two reads of one
/// period can disagree about the number.
```

The two sibling versioned resources that refuse to freeze their membership — `src/state.rs:1292-1293`:
```
/// one copy of them, and `Size` is derived per period from the plan's associations
/// ([`PricingPlanRecord::associated_rule_arns`]) rather than frozen into a generation.
```
and `src/state.rs:1700-1702`:
```
/// so there is one copy of them. The membership a period reports is not here either: it
/// is kept per period on the account memberships themselves
/// ([`ComAmazonawsBillingconductorAwsbillingconductorState::billing_group_sizes_in`]).
```

The reader fed from the stale copy — `src/ops/list_custom_line_items.rs:226`:
```
        association_size: version.association_size(billing_period),
```
and `src/ops/list_custom_line_item_versions.rs:217`:
```
        association_size: version.association_size(&start_billing_period),
```

The readers fed from the LIVE records — `src/ops/list_resources_associated_to_custom_line_item.rs:212-217`:
```
        for association in &item.associated_resources {
            // An association is listed for a period its range covers, the start
            // inclusive and the end exclusive.
            if !association.covers(&billing_period) {
                continue;
            }
```
and `src/state.rs:4124-4129` (`charge_input`, behind both cost reports):
```
    let associated = item
        .associated_resources
        .iter()
        .filter(|association| association.covers(billing_period))
        .map(|association| association.arn.as_str())
        .collect();
```

The writes that touch only the live list — `src/ops/batch_disassociate_resources_from_custom_line_item.rs:395`:
```
                match apply_disassociation(&mut target.associated_resources, index, &removed) {
```
`src/ops/batch_associate_resources_to_custom_line_item.rs:428-432`:
```
            target
                .associated_resources
                .push(crate::state::ResourceAssociationRecord::new(
                    resource_arn, added,
                ));
```
and `src/state.rs:3384-3387` (`remove_custom_line_item`, whose own doc at `:3376-3378` says "the associations other percentage custom line items held TO it (an association names a resource that exists, and its own associations go with the record)"):
```
        for item in self.custom_line_items.values_mut() {
            item.associated_resources
                .retain(|association| association.arn != item_arn);
        }
```

Test coverage: none reaches the case. `tests/all/custom_line_item_associations_accumulate.rs`, `tests/all/percentage_custom_line_item_resource_associations.rs` and `tests/all/resource_association_billing_periods.rs` exercise associations only on items with a single generation (so `version` is `current_version()`, which clones the live list); `tests/all/custom_line_item_versions_by_billing_period.rs` and `tests/all/update_custom_line_item_billing_period_window.rs` create superseded generations but assert no `AssociationSize` against them, and `tests/all/membership_writes_keep_superseded_generations.rs` covers the membership writes of billing groups and pricing plans only — not the two batch resource-association writes of a custom line item.

## Steps to Reproduce

1. Designate the emulated account as the payer so the control plane is reachable (set `ComAmazonawsBillingconductorAwsbillingconductorState::payer_account_id`).
2. POST /create-pricing-plan and POST /create-billing-group to get a billing group G. Call the current billing period M and the previous one M-1.
3. POST /create-custom-line-item against G with ChargeDetails {"Type":"FEE","Flat":{"ChargeValue":100}} — a flat child item C.
4. POST /create-custom-line-item against G with ChargeDetails {"Type":"FEE","Percentage":{"PercentageValue":10,"AssociatedValues":["<C>"]}} and BillingPeriodRange {"InclusiveStartBillingPeriod":"M-1"} (accepted: M-1 is the previous billing period) — a percentage item P applying from M-1 on, with one association to C in effect from M-1 on.
5. POST /update-custom-line-item with {"Arn":"<P>","BillingPeriodRange":{"InclusiveStartBillingPeriod":"M"}} — this opens a new generation from M on and FREEZES the first generation holding exactly [M-1, M), together with a copy of its association to C.
6. POST /delete-custom-line-item with {"Arn":"<C>"} — `remove_custom_line_item` strips C from P's live `associated_resources`, but not from the frozen generation's copy.
7. POST /list-resources-associated-to-custom-line-item with {"Arn":"<P>","BillingPeriod":"M-1"} — an empty AssociatedResources: the live records hold no association to C any more.
8. POST /list-custom-line-items with {"BillingPeriod":"M-1"} — P is reported with AssociationSize 1 for that same period, counting a custom line item that exists nowhere; POST /list-custom-line-item-versions with {"Arn":"<P>"} reports the same 1 for that generation, while POST /list-billing-group-cost-reports with {"BillingPeriod":"M-1"} prices P from the live (empty) associations.
9. Equivalent second path, without deleting anything: after step 5, PUT /batch-disassociate-resources-from-custom-line-item with {"TargetArn":"<P>","ResourceArns":["<C>"],"BillingPeriodRange":{"InclusiveStartBillingPeriod":"M-1"}} (accepted: M-1's bill is still open), then repeat steps 7-8 — the association is gone from the live record and still counted by AssociationSize for M-1.
10. Compare the sibling resources: an AssociatePricingRules/DisassociatePricingRules write and an AssociateAccounts/DisassociateAccounts write are reported per period from the live association records (`PricingPlanRecord::associated_rule_arns`, `billing_group_sizes_in`), so no frozen copy of theirs can drift.
