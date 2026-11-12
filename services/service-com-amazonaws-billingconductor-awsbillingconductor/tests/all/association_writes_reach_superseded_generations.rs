// SPDX-License-Identifier: CC0-1.0

//! A write to a custom line item's resource associations made AFTER a generation of the
//! item was superseded, end to end over the wire: `CreateCustomLineItem` ->
//! `BatchAssociateResourcesToCustomLineItem` -> `UpdateCustomLineItem` (which freezes the
//! generation) -> `BatchAssociateResourcesToCustomLineItem` /
//! `BatchDisassociateResourcesFromCustomLineItem` / `DeleteCustomLineItem` ->
//! `ListCustomLineItems`, `ListCustomLineItemVersions` and
//! `ListResourcesAssociatedToCustomLineItem`.
//!
//! RULE: a resource association is held once, by the custom line item itself, and carries
//! the billing periods it is in effect over — so `AssociationSize`, on EVERY read that
//! reports it and for whichever generation of the item answers the billing period being
//! read, is the number of those associations in effect in that period: the same
//! membership `ListResourcesAssociatedToCustomLineItem` reports for it. A write to the
//! associations is therefore visible to a read of a period a superseded generation answers
//! for, exactly as it is to a read of the current one.
//!
//! The rows span the writes the rule generalises over rather than one call: an association
//! added after the freeze (reaching back over the frozen generation's periods, and from
//! the current period on only), a disassociation (bounded to the frozen generation's
//! period, and unbounded), the outright delete of an associated resource and a delete
//! bounded to the frozen generation's period — plus the control row that writes nothing.
//! `tests/all/association_size_is_billing_period_scoped.rs` owns the same count for writes
//! made BEFORE any generation was frozen — and the harness this suite drives rather than
//! copying — and `tests/all/membership_writes_keep_superseded_generations.rs` the sibling
//! resources' membership writes.

use crate::association_size_is_billing_period_scoped::{
    associate_ok, associated_count, association_size, disassociate_ok, range, target_item,
    update_ok, versions_ok,
};
use crate::percentage_custom_line_item_resource_associations::{period, Emulator};
use serde_json::{json, Value};

/// The write a row makes to the associations once the first generation has been frozen.
enum Write {
    /// Nothing at all: the control row.
    Nothing,
    /// Associate ONE further resource over this `BillingPeriodRange` (`None` for the call
    /// that names none, in effect from the period it lands in on).
    Associate(Option<Value>),
    /// Disassociate the FIRST associated resource over this `BillingPeriodRange`.
    Disassociate(Option<Value>),
    /// Delete the FIRST associated resource, optionally bounded to a `BillingPeriodRange`.
    DeleteResource(Option<Value>),
}

/// One row: how many resources the item holds before its first generation is frozen, the
/// write made to those associations afterwards, and the `AssociationSize` every read of
/// each billing period must report once it is served.
struct Row {
    case: &'static str,
    associated: usize,
    write: Write,
    /// (the billing period a read asks about, the count it must report). The previous
    /// period is the one the FROZEN generation answers for and the current one the
    /// generation in effect, and each is also that generation's `StartBillingPeriod`.
    expected: Vec<(String, i64)>,
}

/// THE RULE: `AssociationSize` is counted from the item's own association records for the
/// billing period the element reports, so a write to those records changes what a read of
/// a period a superseded generation answers for reports, and no two reads of one period
/// disagree about the number.
#[tokio::test]
async fn association_writes_are_visible_through_a_superseded_generation() {
    let rows: Vec<Row> = vec![
        Row {
            case: "no write at all after the generation was frozen",
            associated: 1,
            write: Write::Nothing,
            expected: vec![(period(-1), 1), (period(0), 1)],
        },
        Row {
            case: "a resource associated afterwards over the frozen generation's period",
            associated: 1,
            write: Write::Associate(Some(range(&period(-1), None))),
            expected: vec![(period(-1), 2), (period(0), 2)],
        },
        Row {
            case: "a resource associated afterwards from the current period on only",
            associated: 1,
            write: Write::Associate(None),
            expected: vec![(period(-1), 1), (period(0), 2)],
        },
        Row {
            case: "a disassociation bounded to the frozen generation's period",
            associated: 2,
            write: Write::Disassociate(Some(range(&period(-1), Some(&period(0))))),
            expected: vec![(period(-1), 1), (period(0), 2)],
        },
        Row {
            case: "an unbounded disassociation, in effect from the period it lands in on",
            associated: 1,
            write: Write::Disassociate(None),
            expected: vec![(period(-1), 1), (period(0), 0)],
        },
        Row {
            case: "the associated resource deleted outright",
            associated: 2,
            write: Write::DeleteResource(None),
            expected: vec![(period(-1), 1), (period(0), 1)],
        },
        Row {
            case: "a delete bounded to the frozen generation's period",
            associated: 1,
            write: Write::DeleteResource(Some(range(&period(-1), Some(&period(0))))),
            expected: vec![(period(-1), 0), (period(0), 1)],
        },
    ];

    for row in rows {
        let case = row.case;
        let emulator = Emulator::new();
        let group = emulator.billing_group("home").await;
        // The target, and every resource associated to it, applies from the PREVIOUS
        // billing period on: so the update below leaves the target with a frozen
        // generation answering for that period and the generation in effect answering for
        // the current one, and a delete bounded to that period has periods of a resource
        // to take.
        let target = target_item(&emulator, case, &group, "target").await;
        let wanted = row.associated + usize::from(matches!(row.write, Write::Associate(_)));
        let mut pool: Vec<String> = Vec::new();
        while pool.len() < wanted {
            let name = format!("child{}", pool.len());
            pool.push(target_item(&emulator, case, &group, &name).await);
        }
        associate_ok(
            &emulator,
            case,
            &target,
            &pool[..row.associated],
            Some(range(&period(-1), None)),
        )
        .await;

        // The write that freezes the first generation: applied from the CURRENT billing
        // period, it leaves the generation before it holding exactly the previous one.
        update_ok(
            &emulator,
            case,
            &target,
            json!({
                "Description": "applied from the current billing period",
                "BillingPeriodRange": range(&period(0), None),
            }),
        )
        .await;
        let generations = version_sizes(&emulator, case, &target).await;
        let starts: Vec<String> = generations.iter().map(|(start, _)| start.clone()).collect();
        assert_eq!(
            starts,
            vec![period(-1), period(0)],
            "case {case:?}: expected the frozen generation and the one in effect, got \
             {generations:?}"
        );

        match &row.write {
            Write::Nothing => {}
            Write::Associate(billing_period_range) => {
                associate_ok(
                    &emulator,
                    case,
                    &target,
                    &pool[row.associated..],
                    billing_period_range.clone(),
                )
                .await;
            }
            Write::Disassociate(billing_period_range) => {
                disassociate_ok(
                    &emulator,
                    case,
                    &target,
                    &pool[..1],
                    billing_period_range.clone(),
                )
                .await;
            }
            Write::DeleteResource(billing_period_range) => {
                delete_ok(&emulator, case, &pool[0], billing_period_range.clone()).await;
            }
        }

        let reported = version_sizes(&emulator, case, &target).await;
        for (billing_period, expected) in &row.expected {
            // The read of the item scoped to that period.
            let size = association_size(&emulator, case, &target, billing_period).await;
            assert_eq!(
                size, *expected,
                "case {case:?}: expected an AssociationSize of {expected} in {billing_period}, \
                 got {size}"
            );
            // The listing of the very same associations, for the very same period.
            let listed = associated_count(&emulator, case, &target, billing_period).await;
            assert_eq!(
                listed, *expected,
                "case {case:?}: expected the {expected} associations counted for \
                 {billing_period} to be the {listed} the association listing reports"
            );
            // And the generation whose StartBillingPeriod is that period.
            let version = reported
                .iter()
                .find(|(start, _)| start == billing_period)
                .map(|(_, size)| *size)
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected a generation starting in {billing_period}, got \
                         {reported:?}"
                    )
                });
            assert_eq!(
                version, *expected,
                "case {case:?}: expected the generation starting in {billing_period} to count \
                 {expected} associations, got {version}"
            );
        }
    }
}

/// `DeleteCustomLineItem`, optionally bounded to a `BillingPeriodRange`
/// (`smithy_query(query="operation",operation="DeleteCustomLineItem")`: `Arn` and
/// `BillingPeriodRange`), asserted to succeed.
async fn delete_ok(
    emulator: &Emulator,
    case: &str,
    arn: &str,
    billing_period_range: Option<Value>,
) {
    let mut body = json!({ "Arn": arn });
    if let Some(billing_period_range) = billing_period_range {
        body["BillingPeriodRange"] = billing_period_range;
    }
    let (status, answered) = emulator
        .send("POST", "/delete-custom-line-item", &body)
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the delete of {arn} to answer HTTP 200, got {status} with \
         {answered}"
    );
}

/// The `(StartBillingPeriod, AssociationSize)` of every generation
/// `ListCustomLineItemVersions` reports for one item, oldest first — read through the
/// version listing the sibling suite already drives.
///
/// `AssociationSize` is a long with a modelled default of `0`
/// (`smithy_query(query="shape",shape="CustomLineItemVersionListElement")`) and a
/// generation always has a start billing period, so an element missing either member is a
/// failure here rather than a default that could pass a row expecting none.
async fn version_sizes(emulator: &Emulator, case: &str, target: &str) -> Vec<(String, i64)> {
    versions_ok(emulator, case, target)
        .await
        .iter()
        .map(|version| {
            let start = version
                .get("StartBillingPeriod")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a StartBillingPeriod, got {version}")
                })
                .to_string();
            let size = version
                .get("AssociationSize")
                .and_then(Value::as_i64)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected an AssociationSize, got {version}")
                });
            (start, size)
        })
        .collect()
}
