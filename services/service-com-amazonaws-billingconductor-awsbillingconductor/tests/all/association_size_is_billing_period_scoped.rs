// SPDX-License-Identifier: CC0-1.0

//! What `AssociationSize` counts on every read that reports it, end to end over the
//! wire: `CreateBillingGroup` -> `CreateCustomLineItem` ->
//! `BatchAssociateResourcesToCustomLineItem` /
//! `BatchDisassociateResourcesFromCustomLineItem` -> `ListCustomLineItems`,
//! `ListResourcesAssociatedToCustomLineItem`, `UpdateCustomLineItem` and
//! `ListCustomLineItemVersions`.
//!
//! RULE: `AssociationSize` — "The number of resources that are associated to the custom
//! line item" — is counted FOR the billing period the element carrying it is scoped to:
//! it is the number of resources whose association is in effect in that period, the
//! half-open `[InclusiveStartBillingPeriod, ExclusiveEndBillingPeriod)` range each
//! association record holds. So it is the number of resources
//! `ListResourcesAssociatedToCustomLineItem` reports for the same period, and a resource
//! the item holds a record for counts in the periods that record covers and in no other.
//!
//! The rows span what the rule generalises over rather than one request: the bounds of
//! the association's own range (none at all, a later start, an earlier start, bounded at
//! both ends), how many resources are associated and over which ranges, what a bounded
//! and an unbounded disassociation leave of a record it keeps, the spelling of the months
//! (`2026-4` / `2026-04`), and the billing period each read asks about — before the
//! association, inside it, at its exclusive end and past it.

use crate::percentage_custom_line_item_resource_associations::{one_digit_month, period, Emulator};
use serde_json::{json, Value};

/// A `CustomLineItemBillingPeriodRange`, as the model shapes it
/// (`smithy_query(query="shape",shape="CustomLineItemBillingPeriodRange")`: `@required
/// InclusiveStartBillingPeriod`, optional `ExclusiveEndBillingPeriod`).
///
/// `pub(crate)` so the sibling suite about the same count drives these helpers rather
/// than keeping a second copy of them that could drift (see
/// `tests/all/association_writes_reach_superseded_generations.rs`).
pub(crate) fn range(inclusive_start: &str, exclusive_end: Option<&str>) -> Value {
    let mut value = json!({ "InclusiveStartBillingPeriod": inclusive_start });
    if let Some(end) = exclusive_end {
        value["ExclusiveEndBillingPeriod"] = json!(end);
    }
    value
}

/// A PERCENTAGE custom line item — the only kind resources can be associated to — in
/// effect from the PREVIOUS billing period on, so that a read of that period reports it
/// and the count it reports there can be asked about at all.
pub(crate) async fn target_item(
    emulator: &Emulator,
    case: &str,
    group: &str,
    name: &str,
) -> String {
    let (status, body) = emulator
        .send(
            "POST",
            "/create-custom-line-item",
            &json!({
                "Name": name,
                "Description": "the item whose associations are counted",
                "BillingGroupArn": group,
                "ChargeDetails": { "Type": "FEE", "Percentage": { "PercentageValue": 10.0 } },
                "BillingPeriodRange": range(&period(-1), None),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the create to answer HTTP 200, got {status} with {body}"
    );
    body.get("Arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an Arn, got {body}"))
        .to_string()
}

/// The `AssociationSize` `ListCustomLineItems` reports for one item in `billing_period`.
pub(crate) async fn association_size(
    emulator: &Emulator,
    case: &str,
    target: &str,
    billing_period: &str,
) -> i64 {
    let (status, body) = emulator
        .send(
            "POST",
            "/list-custom-line-items",
            &json!({
                "BillingPeriod": billing_period,
                "Filters": { "Arns": [target] },
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the item read for {billing_period} to answer HTTP 200, got \
         {status} with {body}"
    );
    body.get("CustomLineItems")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("AssociationSize"))
        .and_then(Value::as_i64)
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected an AssociationSize for {billing_period}, got {body}")
        })
}

/// How many resources `ListResourcesAssociatedToCustomLineItem` reports for one item in
/// `billing_period`: the membership `AssociationSize` has to agree with.
pub(crate) async fn associated_count(
    emulator: &Emulator,
    case: &str,
    target: &str,
    billing_period: &str,
) -> i64 {
    let reported = emulator
        .association_pairs(
            target,
            json!({ "BillingPeriod": billing_period, "MaxResults": 100 }),
        )
        .await;
    i64::try_from(reported.len()).unwrap_or_else(|_| {
        panic!("case {case:?}: expected the reported membership of {billing_period} to fit")
    })
}

/// `BatchAssociateResourcesToCustomLineItem` over `resources`, asserted to succeed.
pub(crate) async fn associate_ok(
    emulator: &Emulator,
    case: &str,
    target: &str,
    resources: &[String],
    billing_period_range: Option<Value>,
) {
    let (status, body) = emulator
        .associate(target, resources, billing_period_range)
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the association to answer HTTP 200, got {status} with {body}"
    );
    assert_eq!(
        body.get("FailedAssociatedResources"),
        Some(&json!([])),
        "case {case:?}: expected no failed resource, got {body}"
    );
}

/// `BatchDisassociateResourcesFromCustomLineItem` over `resources`, asserted to succeed.
pub(crate) async fn disassociate_ok(
    emulator: &Emulator,
    case: &str,
    target: &str,
    resources: &[String],
    billing_period_range: Option<Value>,
) {
    let mut body = json!({ "TargetArn": target, "ResourceArns": resources });
    if let Some(billing_period_range) = billing_period_range {
        body["BillingPeriodRange"] = billing_period_range;
    }
    let (status, answered) = emulator
        .send(
            "PUT",
            "/batch-disassociate-resources-from-custom-line-item",
            &body,
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the disassociation to answer HTTP 200, got {status} with \
         {answered}"
    );
    assert_eq!(
        answered.get("FailedDisassociatedResources"),
        Some(&json!([])),
        "case {case:?}: expected no failed resource, got {answered}"
    );
}

/// One row: the calls that build the item's association records, and the count every
/// read scoped to each probed billing period must report.
struct Row {
    case: &'static str,
    /// One entry per associating call: how many resources it names, and the
    /// `BillingPeriodRange` it carries (`None` for the call that names none, which is in
    /// effect from the period it lands in on).
    associations: Vec<(usize, Option<Value>)>,
    /// A disassociation of EVERY associated resource applied afterwards, if any: `None`
    /// for a row that makes none, `Some(None)` for the call that names no range, and
    /// `Some(Some(range))` for a bounded one.
    disassociated: Option<Option<Value>>,
    /// (the billing period a read asks about, the `AssociationSize` it must report)
    probes: Vec<(String, i64)>,
}

/// THE RULE: every read of a custom line item scoped to a billing period reports, as
/// `AssociationSize`, the number of resources whose association is in effect in THAT
/// period — the same membership `ListResourcesAssociatedToCustomLineItem` reports there.
#[tokio::test]
async fn association_size_counts_the_associations_in_effect_in_the_period_read() {
    let rows: Vec<Row> = vec![
        Row {
            case: "an association with no range of its own",
            associations: vec![(1, None)],
            disassociated: None,
            // In effect from the period the associating call landed in on, so the
            // previous period — which the item itself applies in — counts nothing.
            probes: vec![(period(-1), 0), (period(0), 1), (period(3), 1)],
        },
        Row {
            case: "an association beginning only in a later period",
            associations: vec![(1, Some(range(&period(2), None)))],
            disassociated: None,
            probes: vec![
                (period(-1), 0),
                (period(0), 0),
                (period(1), 0),
                (period(2), 1),
                (period(9), 1),
            ],
        },
        Row {
            case: "an association reaching back to the previous period",
            associations: vec![(1, Some(range(&period(-1), None)))],
            disassociated: None,
            probes: vec![(period(-1), 1), (period(0), 1), (period(4), 1)],
        },
        Row {
            case: "an association bounded at both ends",
            associations: vec![(1, Some(range(&period(-1), Some(&period(1)))))],
            disassociated: None,
            // The end bound is EXCLUSIVE, so the period it names counts nothing.
            probes: vec![
                (period(-1), 1),
                (period(0), 1),
                (period(1), 0),
                (period(2), 0),
            ],
        },
        Row {
            case: "two resources whose ranges differ",
            associations: vec![(1, None), (1, Some(range(&period(2), None)))],
            disassociated: None,
            probes: vec![
                (period(-1), 0),
                (period(0), 1),
                (period(1), 1),
                (period(2), 2),
                (period(3), 2),
            ],
        },
        Row {
            case: "three resources, one of them ending before the others begin",
            associations: vec![
                (1, Some(range(&period(-1), Some(&period(0))))),
                (2, Some(range(&period(0), None))),
            ],
            disassociated: None,
            probes: vec![(period(-1), 1), (period(0), 2), (period(5), 2)],
        },
        Row {
            case: "a bounded disassociation taking the current period out",
            associations: vec![(1, None)],
            disassociated: Some(Some(range(&period(0), Some(&period(1))))),
            probes: vec![(period(0), 0), (period(1), 1), (period(2), 1)],
        },
        Row {
            case: "an unbounded disassociation of an association that began earlier",
            associations: vec![(1, Some(range(&period(-1), None)))],
            disassociated: Some(None),
            probes: vec![(period(-1), 1), (period(0), 0), (period(1), 0)],
        },
        Row {
            case: "a bounded disassociation leaving periods on either side of it",
            associations: vec![(2, Some(range(&period(-1), None)))],
            disassociated: Some(Some(range(&period(0), Some(&period(2))))),
            probes: vec![
                (period(-1), 2),
                (period(0), 0),
                (period(1), 0),
                (period(2), 2),
            ],
        },
        Row {
            case: "a range whose months are spelled without their leading zeroes",
            associations: vec![(
                1,
                Some(range(
                    &one_digit_month(&period(-1)),
                    Some(&one_digit_month(&period(1))),
                )),
            )],
            disassociated: None,
            // The two spellings of one month are one billing period, whichever the call
            // that wrote the range and the read that asks about it used.
            probes: vec![
                (one_digit_month(&period(-1)), 1),
                (period(-1), 1),
                (period(0), 1),
                (one_digit_month(&period(1)), 0),
                (period(1), 0),
            ],
        },
    ];

    for row in rows {
        let case = row.case;
        let emulator = Emulator::new();
        let group = emulator.billing_group("home").await;
        let target = target_item(&emulator, case, &group, "target").await;

        // The resources in scope of the billing group the target applies to: custom line
        // items of that same group.
        let wanted: usize = row.associations.iter().map(|(count, _)| count).sum();
        let mut pool: Vec<String> = Vec::new();
        while pool.len() < wanted {
            pool.push(
                emulator
                    .percentage_item(&group, &format!("child{}", pool.len()))
                    .await,
            );
        }

        let mut next = 0;
        for (count, billing_period_range) in &row.associations {
            let batch = pool[next..next + count].to_vec();
            next += count;
            associate_ok(
                &emulator,
                case,
                &target,
                &batch,
                billing_period_range.clone(),
            )
            .await;
        }
        if let Some(billing_period_range) = row.disassociated.clone() {
            disassociate_ok(&emulator, case, &target, &pool, billing_period_range).await;
        }

        for (billing_period, expected) in &row.probes {
            let size = association_size(&emulator, case, &target, billing_period).await;
            assert_eq!(
                size, *expected,
                "case {case:?}: expected an AssociationSize of {expected} in {billing_period}, \
                 got {size}"
            );
            // The two period-scoped reads of one membership answer alike.
            let listed = associated_count(&emulator, case, &target, billing_period).await;
            assert_eq!(
                listed, size,
                "case {case:?}: expected the {listed} resources reported for {billing_period} to \
                 be the {size} AssociationSize counts"
            );
        }

        // "If you don't provide a billing period, the current billing period is used":
        // the read naming none reports the count of the current period.
        let (status, body) = emulator
            .send(
                "POST",
                "/list-custom-line-items",
                &json!({ "Filters": { "Arns": [&target] } }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the default read to answer HTTP 200, got {status} with {body}"
        );
        let default_size = body
            .get("CustomLineItems")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|item| item.get("AssociationSize"))
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("case {case:?}: expected an AssociationSize, got {body}"));
        let current = association_size(&emulator, case, &target, &period(0)).await;
        assert_eq!(
            default_size, current,
            "case {case:?}: expected the read naming no billing period to count the current \
             period's {current} associations, got {default_size}"
        );
    }
}

/// THE RULE on the other two reads that report the member: `UpdateCustomLineItem` answers
/// with the count of the billing period its `BillingPeriodRange` applies the update from,
/// and each element of `ListCustomLineItemVersions` with the count of the period that
/// generation takes effect in — so neither can disagree with the read of the item for
/// that same period.
#[tokio::test]
async fn the_update_answer_and_the_version_history_count_their_own_billing_period() {
    let case = "associations in effect in one generation's periods only";
    let emulator = Emulator::new();
    let group = emulator.billing_group("home").await;
    let target = target_item(&emulator, case, &group, "target").await;
    let early = emulator.percentage_item(&group, "early").await;
    let later = emulator.percentage_item(&group, "later").await;

    // One resource in effect in the PREVIOUS billing period only, one from two periods
    // out on: so each billing period of the item has a different count, and no period
    // counts both.
    associate_ok(
        &emulator,
        case,
        &target,
        std::slice::from_ref(&early),
        Some(range(&period(-1), Some(&period(0)))),
    )
    .await;
    associate_ok(
        &emulator,
        case,
        &target,
        std::slice::from_ref(&later),
        Some(range(&period(2), None)),
    )
    .await;

    for (billing_period, expected) in [
        (period(-1), 1),
        (period(0), 0),
        (period(1), 0),
        (period(2), 1),
    ] {
        let size = association_size(&emulator, case, &target, &billing_period).await;
        assert_eq!(
            size, expected,
            "case {case:?}: expected an AssociationSize of {expected} in {billing_period}, got \
             {size}"
        );
    }

    // The update applied FROM the current billing period answers with that period's
    // count, which neither association is in effect in.
    let answered = update_ok(
        &emulator,
        case,
        &target,
        json!({
            "Description": "updated from the current period",
            "BillingPeriodRange": range(&period(0), None),
        }),
    )
    .await;
    assert_eq!(
        answered.get("AssociationSize").and_then(Value::as_i64),
        Some(0),
        "case {case:?}: expected the update applied from {} to count its own period's \
         associations, got {answered}",
        period(0)
    );

    // The history: the generation the update froze takes effect in the previous period
    // and counts the association in effect there, the newest one takes effect in the
    // current period and counts none.
    let versions = versions_ok(&emulator, case, &target).await;
    let reported: Vec<(String, i64)> = versions
        .iter()
        .map(|version| {
            (
                version
                    .get("StartBillingPeriod")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                version
                    .get("AssociationSize")
                    .and_then(Value::as_i64)
                    .unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        reported,
        vec![(period(-1), 1), (period(0), 0)],
        "case {case:?}: expected each generation to count the associations in effect in the \
         period it takes effect in, got {versions:?}"
    );
    // And each of those counts is the one a read of the item for that same period reports.
    for (billing_period, counted) in &reported {
        let size = association_size(&emulator, case, &target, billing_period).await;
        assert_eq!(
            size, *counted,
            "case {case:?}: expected the version starting in {billing_period} to count the \
             {size} associations the item read reports there, got {counted}"
        );
    }

    // An update applied from the PREVIOUS billing period answers with THAT period's
    // count instead — the answer is scoped to the period the write applies from, not to
    // the period the call is made in.
    let answered = update_ok(
        &emulator,
        case,
        &target,
        json!({
            "Description": "updated from the previous period",
            "BillingPeriodRange": range(&period(-1), None),
        }),
    )
    .await;
    assert_eq!(
        answered.get("AssociationSize").and_then(Value::as_i64),
        Some(1),
        "case {case:?}: expected the update applied from {} to count that period's one \
         association, got {answered}",
        period(-1)
    );
}

/// `UpdateCustomLineItem` with whatever members the case carries, asserted to succeed.
pub(crate) async fn update_ok(
    emulator: &Emulator,
    case: &str,
    target: &str,
    request: Value,
) -> Value {
    let mut body = json!({ "Arn": target });
    for (member, value) in request.as_object().cloned().unwrap_or_default() {
        body[member] = value;
    }
    let (status, answered) = emulator
        .send("POST", "/update-custom-line-item", &body)
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the update to answer HTTP 200, got {status} with {answered}"
    );
    answered
}

/// The versions `ListCustomLineItemVersions` reports for one item, oldest first.
pub(crate) async fn versions_ok(emulator: &Emulator, case: &str, target: &str) -> Vec<Value> {
    let (status, body) = emulator
        .send(
            "POST",
            "/list-custom-line-item-versions",
            &json!({ "Arn": target, "MaxResults": 100 }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the version read to answer HTTP 200, got {status} with {body}"
    );
    body.get("CustomLineItemVersions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}
