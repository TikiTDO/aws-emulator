// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ORDER scope of `GetOutpostSupportedInstanceTypes`, driven
//! over the wire through the same front door as every other suite (the harness in
//! [`crate::site_and_outpost_lifecycle`]).
//!
//! The rule under test, stated once and then exercised over a SET of inputs: an
//! `OrderId` scopes the report to the hardware THAT order is for, which is the
//! configurations its line items subscribe to. Each of those publishes the instance
//! families it carries EC2 capacity of and the maximum size of each — read off
//! `ListCatalogItems`, which every expectation below is computed from rather than
//! spelled out — so the report is the published instance-type catalog narrowed to
//! those families, no entry larger than the maximum size of its family, intersected
//! with every other hardware fact the scope the request names records: the Outpost's
//! own `SupportedHardwareType`, and one asset's `ComputeAttributes` when an `AssetId`
//! names one.
//!
//! Three things follow and are asserted as part of the rule:
//!
//! * a line item naming a SKU this service publishes no configuration under records no
//!   hardware fact, so it narrows nothing;
//! * what is reported is the CAPABILITY of the hardware the order is for, so it does
//!   not depend on how many units the order bought, on the order of its line items, on
//!   whether that hardware has been installed yet, or on the status the order has
//!   reached;
//! * scoping NARROWS: every entry a scoped report answers is one the Outpost itself
//!   answers, and reading a scoped report changes nothing a later caller sees.
//!
//! What this report answers when no order is named, and what it refuses, is
//! [`crate::supported_instance_types`]; the narrower CONFIGURED report derived from the
//! same catalog records is [`crate::configured_instance_types`].

use serde_json::{json, Value};

use crate::outpost_assets::{asset, compute, record};
use crate::site_and_outpost_lifecycle::{orderable_site, query_value, segment, text, Outposts};

/// A `CatalogItemId` of the modelled `SkuCode` form (`^OR-[A-Z0-9]{7}$`) that this
/// service publishes no hardware configuration under.
const UNPUBLISHED_SKU: &str = "OR-0000000";

/// The modelled `CatalogItemStatus` a published configuration an order can still be
/// placed for carries, as `smithy_query(query="shape", shape="CatalogItemStatus")`
/// reports the value set (`AVAILABLE` | `DISCONTINUED`).
const ORDERABLE_ITEM_STATUS: &str = "AVAILABLE";

/// The modelled `SupportedHardwareType` values (`RACK` | `SERVER`), as
/// `smithy_query(query="shape", shape="SupportedHardwareType")` reports them, plus the
/// absent case — the three hardware types an Outpost in this store can carry.
const HARDWARE_TYPES: &[Option<&str>] = &[Some("RACK"), Some("SERVER"), None];

/// The route `GetOutpostSupportedInstanceTypes` binds, for one identifier form and one
/// query string.
fn route(identifier: &str, query: &str) -> String {
    format!(
        "/outposts/{}/supportedInstanceTypes{query}",
        segment(identifier)
    )
}

/// `GetOutpostSupportedInstanceTypes`, requiring HTTP 200.
async fn supported(outposts: &Outposts, identifier: &str, query: &str) -> Value {
    outposts.ok("GET", &route(identifier, query), None).await
}

/// The report of one call, as the `(InstanceType, VCPUs)` of every entry in the order
/// it reported them.
async fn report(outposts: &Outposts, identifier: &str, query: &str) -> Vec<(String, i64)> {
    reported(&supported(outposts, identifier, query).await)
}

/// The `(InstanceType, VCPUs)` of every entry of one answer, in the order it reported
/// them.
fn reported(output: &Value) -> Vec<(String, i64)> {
    output
        .get("InstanceTypes")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| (text(item, "InstanceType"), vcpus(item)))
                .collect()
        })
        .unwrap_or_default()
}

/// The `VCPUs` one reported or published entry carries, or `-1` when it carries none —
/// which no catalog entry does, so a missing count fails a row rather than being
/// skipped.
fn vcpus(entry: &Value) -> i64 {
    entry.get("VCPUs").and_then(Value::as_i64).unwrap_or(-1)
}

/// Every instance type this service holds a specification of: the source every
/// expectation below is computed from.
///
/// What an Outpost supports is every size its hardware runs, so this is the wider of
/// the service's two instance-type lists rather than the catalog
/// `ListOrderableInstanceTypes` publishes (one entry per family, see
/// [`crate::instance_type_catalogs`]).
fn catalog() -> Vec<Value> {
    crate::instance_type_catalogs::specifications()
}

/// The `VCPUs` that catalog publishes for `instance_type`, or `None` for a type it
/// publishes no entry for.
fn published_vcpus(catalog: &[Value], instance_type: &str) -> Option<i64> {
    catalog
        .iter()
        .find(|entry| text(entry, "InstanceType") == instance_type)
        .map(vcpus)
}

/// Whether a catalog entry's `FormFactorConfigs` offers it in `form_factor`.
fn offered_in(entry: &Value, form_factor: &str) -> bool {
    entry
        .get("FormFactorConfigs")
        .and_then(Value::as_array)
        .is_some_and(|configs| {
            configs
                .iter()
                .any(|config| text(config, "FormFactor") == form_factor)
        })
}

/// The instance family of a catalog entry: `c6id` in `c6id.2xlarge`.
fn family(entry: &Value) -> String {
    text(entry, "InstanceType")
        .split('.')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Every configuration an order can be placed FOR, in the order `ListCatalogItems`
/// publishes them: the catalog items it publishes in the status a configuration still
/// being offered carries ([`ORDERABLE_ITEM_STATUS`]), read off the catalog's own
/// `ItemStatus` rather than spelled out here.
async fn orderable_configurations(outposts: &Outposts) -> Vec<Value> {
    let published = outposts
        .ok("GET", "/catalog/items?MaxResults=1000", None)
        .await["CatalogItems"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        !published
            .iter()
            .any(|item| text(item, "CatalogItemId") == UNPUBLISHED_SKU),
        "the rows that order {UNPUBLISHED_SKU:?} need a SKU this service publishes nothing \
         under, got {published:?}"
    );
    published
        .into_iter()
        .filter(|item| text(item, "ItemStatus") == ORDERABLE_ITEM_STATUS)
        .collect()
}

/// The EC2 capacity the hardware one order is for carries, as `(Family, vCPUs of
/// MaxSize)` pairs: what the rule narrows the catalog by, computed from the
/// configurations the order's line items name.
fn ordered_capacity(catalog: &[Value], configurations: &[&Value]) -> Vec<(String, i64)> {
    let mut ordered: Vec<(String, i64)> = Vec::new();
    for item in configurations {
        for capacity in item
            .get("EC2Capacities")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let (family, max_size) = (text(capacity, "Family"), text(capacity, "MaxSize"));
            // `Family` names the family and `MaxSize` the size alone, so the instance
            // type the capacity names is the join of the two.
            let instance_type = format!("{family}.{max_size}");
            let max_vcpus = published_vcpus(catalog, &instance_type).unwrap_or_else(|| {
                panic!(
                    "the published catalog must carry {instance_type:?}, the maximum size of \
                     {family:?} a configuration carries capacity of"
                )
            });
            let entry = (family, max_vcpus);
            if !ordered.contains(&entry) {
                ordered.push(entry);
            }
        }
    }
    ordered
}

/// The hardware facts the scope a request names records: what the rule narrows the
/// catalog by.
struct Scope<'a> {
    /// The Outpost's `SupportedHardwareType`, or `None` for an Outpost that recorded
    /// none.
    form_factor: Option<&'a str>,
    /// The EC2 capacity the order in scope bought ([`ordered_capacity`]), empty where
    /// the request names no order or the order names no published configuration.
    ordered: Vec<(String, i64)>,
    /// The instance families the scoped asset records, or `None` when the request
    /// names no asset or the asset recorded none.
    families: Option<&'a [&'a str]>,
    /// The scoped asset's `MaxVcpus`, or `None` when none is recorded.
    max_vcpus: Option<i64>,
    /// Whether the scope is an asset that is NOT compute hardware, which can host no
    /// instance at all.
    not_compute: bool,
}

impl<'a> Scope<'a> {
    /// The Outpost and one order of it: the scope every row of the order table names.
    fn ordered(form_factor: Option<&'a str>, ordered: Vec<(String, i64)>) -> Self {
        Self {
            form_factor,
            ordered,
            families: None,
            max_vcpus: None,
            not_compute: false,
        }
    }
}

/// What the rule requires for `scope`, computed from the published catalog.
fn expected(catalog: &[Value], scope: &Scope<'_>) -> Vec<(String, i64)> {
    if scope.not_compute {
        return Vec::new();
    }
    catalog
        .iter()
        .filter(|entry| {
            scope
                .form_factor
                .is_none_or(|form_factor| offered_in(entry, form_factor))
                && (scope.ordered.is_empty()
                    || scope.ordered.iter().any(|(ordered_family, max_vcpus)| {
                        *ordered_family == family(entry) && vcpus(entry) <= *max_vcpus
                    }))
                && scope
                    .families
                    .is_none_or(|families| families.contains(&family(entry).as_str()))
                && scope.max_vcpus.is_none_or(|max| vcpus(entry) <= max)
        })
        .map(|entry| (text(entry, "InstanceType"), vcpus(entry)))
        .collect()
}

/// One site and one Outpost on it carrying `hardware_type`, as that Outpost's
/// `OutpostId`.
async fn an_outpost(outposts: &Outposts, name: &str, hardware_type: Option<&str>) -> String {
    let site = outposts
        .create_site(&orderable_site(&format!("{name}-site")))
        .await;
    let mut request = json!({ "Name": name, "SiteId": text(&site, "SiteId") });
    if let Some(hardware_type) = hardware_type {
        request["SupportedHardwareType"] = json!(hardware_type);
    }
    text(&outposts.create_outpost(&request).await, "OutpostId")
}

/// `CreateOrder` on `outpost_id` for `line_items` — each a `(CatalogItemId, Quantity)`
/// pair — as the `OrderId` it minted.
async fn an_order(outposts: &Outposts, outpost_id: &str, line_items: &[(String, i64)]) -> String {
    let requested: Vec<Value> = line_items
        .iter()
        .map(|(catalog_item_id, quantity)| {
            json!({ "CatalogItemId": catalog_item_id, "Quantity": quantity })
        })
        .collect();
    let placed = outposts
        .ok(
            "POST",
            "/orders",
            Some(&json!({
                "OutpostIdentifier": outpost_id,
                "PaymentOption": "ALL_UPFRONT",
                "LineItems": requested,
            })),
        )
        .await;
    text(&placed["Order"], "OrderId")
}

/// The query string that scopes the report to one order.
fn by_order(order_id: &str) -> String {
    format!("?OrderId={}", query_value(order_id))
}

/// Walk an order-scoped report `size` entries at a time, following every `NextToken` to
/// the end.
async fn walk(
    outposts: &Outposts,
    outpost_id: &str,
    order_id: &str,
    size: usize,
    ceiling: usize,
) -> Vec<(String, i64)> {
    let mut collected: Vec<(String, i64)> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let query = match &token {
            Some(token) => format!(
                "?OrderId={}&MaxResults={size}&NextToken={}",
                query_value(order_id),
                query_value(token)
            ),
            None => format!("?OrderId={}&MaxResults={size}", query_value(order_id)),
        };
        let page = supported(outposts, outpost_id, &query).await;
        let entries = reported(&page);
        assert!(
            entries.len() <= size,
            "case size {size}: a page holds at most MaxResults entries, got {entries:?}"
        );
        collected.extend(entries);
        pages += 1;
        assert!(
            pages <= ceiling + 1,
            "case size {size}: paging must terminate, got {pages} pages and {collected:?}"
        );
        match page.get("NextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    collected
}

/// The rule over EVERY configuration this service publishes an order can be placed
/// for, on an Outpost of each hardware type one can carry: the report scoped to an
/// order of that configuration is the catalog narrowed to the families it carries EC2
/// capacity of, intersected with the Outpost's own form factor.
///
/// The rows are the published configurations themselves, so the table spans the
/// catalog rather than the one SKU a report cited, and every order is placed on the
/// SAME Outpost — which is what makes two `OrderId`s of one Outpost two different
/// questions.
#[tokio::test]
async fn an_order_scopes_the_report_to_the_capacity_its_configurations_carry() {
    let outposts = Outposts::new();
    let catalog = catalog();
    let configurations = orderable_configurations(&outposts).await;
    assert!(
        configurations.len() >= 2,
        "the published catalog must offer configurations to order, got {configurations:?}"
    );

    for hardware_type in HARDWARE_TYPES.iter().copied() {
        let outpost_id = an_outpost(
            &outposts,
            &format!("ordered-{}", hardware_type.unwrap_or("none")),
            hardware_type,
        )
        .await;
        // What an order narrows: the catalog this Outpost's form factor offers, which
        // is what an order recording no hardware fact answers. The Outpost's OWN
        // report is about the hardware installed on it — nothing is, here — so the
        // comparison is against this computed scope and not against that report (see
        // [`crate::supported_instance_types`]).
        let unscoped = expected(&catalog, &Scope::ordered(hardware_type, Vec::new()));
        assert!(
            !unscoped.is_empty(),
            "case {hardware_type:?}: the form factor must offer something to narrow, got \
             {unscoped:?}"
        );

        let mut answers: Vec<Vec<(String, i64)>> = Vec::new();
        for item in &configurations {
            let sku = text(item, "CatalogItemId");
            let case = format!("{sku} on {hardware_type:?}");
            let order_id = an_order(&outposts, &outpost_id, &[(sku.clone(), 1)]).await;
            let want = expected(
                &catalog,
                &Scope::ordered(hardware_type, ordered_capacity(&catalog, &[item])),
            );
            let answer = report(&outposts, &outpost_id, &by_order(&order_id)).await;
            assert_eq!(
                answer, want,
                "case {case:?}: expected the catalog narrowed to the capacity that order \
                 bought, got {answer:?}"
            );
            assert!(
                answer.iter().all(|entry| unscoped.contains(entry)),
                "case {case:?}: an order scope must narrow, got {answer:?} against {unscoped:?}"
            );
            answers.push(answer);
        }

        // Two orders of DIFFERENT configurations on one Outpost are two different
        // reports: the member is what tells them apart.
        let distinct: std::collections::BTreeSet<&Vec<(String, i64)>> = answers.iter().collect();
        assert!(
            distinct.len() >= 2,
            "case {hardware_type:?}: orders of different configurations must be answered \
             differently, got {answers:?}"
        );
        assert!(
            answers.iter().any(|answer| !answer.is_empty()),
            "case {hardware_type:?}: an order of hardware this Outpost's form factor offers \
             must report something, got {answers:?}"
        );
    }
}

/// One row of the order table: the line items an order carries, and what the row's own
/// expectation is of the answer.
struct Row {
    /// What this row is a case of, named in every assertion it makes.
    case: &'static str,
    /// The line items the order carries: an index into the two configurations the test
    /// picked, or `None` for [`UNPUBLISHED_SKU`], with how many units of it the order
    /// buys.
    items: &'static [(Option<usize>, i64)],
    /// Whether the order is cancelled before the report is read.
    cancelled: bool,
}

/// The rule over the shapes an order can take: how many line items it carries, how
/// many units of each, which order they are named in, a SKU this service publishes
/// nothing under beside a published one, and an order that was cancelled.
#[tokio::test]
async fn what_an_order_is_for_is_its_published_line_items_and_nothing_else() {
    let outposts = Outposts::new();
    let catalog = catalog();
    let configurations = orderable_configurations(&outposts).await;
    // Two configurations whose EC2 capacity names DIFFERENT families, so an order of
    // both is a wider scope than an order of either: which two is read off the
    // published catalog rather than spelled out.
    let chosen: Vec<&Value> = {
        let first = configurations.first().expect("a published configuration");
        let other = configurations
            .iter()
            .find(|item| {
                ordered_capacity(&catalog, &[item]) != ordered_capacity(&catalog, &[first])
            })
            .expect("two published configurations that carry capacity of different families");
        vec![first, other]
    };

    let rows: &[Row] = &[
        Row {
            case: "one configuration, one unit of it",
            items: &[(Some(0), 1)],
            cancelled: false,
        },
        Row {
            case: "several units of one configuration",
            items: &[(Some(0), 3)],
            cancelled: false,
        },
        Row {
            case: "two configurations at once",
            items: &[(Some(0), 1), (Some(1), 1)],
            cancelled: false,
        },
        Row {
            case: "the same two, named the other way round",
            items: &[(Some(1), 2), (Some(0), 1)],
            cancelled: false,
        },
        Row {
            case: "a SKU this service publishes nothing under",
            items: &[(None, 1)],
            cancelled: false,
        },
        Row {
            case: "an unpublished SKU beside a published configuration",
            items: &[(None, 1), (Some(0), 1)],
            cancelled: false,
        },
        Row {
            case: "a cancelled order",
            items: &[(Some(0), 1)],
            cancelled: true,
        },
    ];

    // Every row is read on ONE Outpost, so what differs between the answers is the
    // order each one names and nothing else. What an order narrows is the catalog this
    // Outpost's form factor offers, which is what an order recording no hardware fact
    // answers.
    let outpost_id = an_outpost(&outposts, "order-shapes", Some("RACK")).await;
    let unscoped = expected(&catalog, &Scope::ordered(Some("RACK"), Vec::new()));
    let mut answers: Vec<Vec<(String, i64)>> = Vec::new();
    for row in rows {
        let case = row.case;
        let line_items: Vec<(String, i64)> = row
            .items
            .iter()
            .map(|(chosen_index, quantity)| {
                let catalog_item_id = match chosen_index {
                    Some(index) => text(chosen[*index], "CatalogItemId"),
                    None => UNPUBLISHED_SKU.to_string(),
                };
                (catalog_item_id, *quantity)
            })
            .collect();
        let order_id = an_order(&outposts, &outpost_id, &line_items).await;
        if row.cancelled {
            outposts
                .ok(
                    "POST",
                    &format!("/orders/{}/cancel", segment(&order_id)),
                    None,
                )
                .await;
        }
        let published: Vec<&Value> = row
            .items
            .iter()
            .filter_map(|(chosen_index, _)| chosen_index.map(|index| chosen[index]))
            .collect();
        let want = expected(
            &catalog,
            &Scope::ordered(Some("RACK"), ordered_capacity(&catalog, &published)),
        );
        // The expectation computed from the published catalog has to say something: an
        // order of no published configuration narrows nothing, one of a single
        // published configuration narrows strictly (no one configuration carries
        // capacity of every family a form factor offers), and neither can pass by
        // expecting nothing at all.
        if published.is_empty() {
            assert_eq!(
                want, unscoped,
                "case {case:?}: an order of no published configuration must expect the \
                 catalog this Outpost's form factor offers"
            );
        } else {
            assert!(
                !want.is_empty(),
                "case {case:?}: an order of a published configuration must expect entries"
            );
        }
        if published.len() == 1 {
            assert_ne!(
                want, unscoped,
                "case {case:?}: one published configuration must narrow the catalog, got all \
                 {} entries",
                unscoped.len()
            );
        }
        let answer = report(&outposts, &outpost_id, &by_order(&order_id)).await;
        assert_eq!(
            answer, want,
            "case {case:?}: expected the catalog narrowed to the capacity that order bought, \
             got {answer:?}"
        );
        // A read: asking again answers the same thing.
        assert_eq!(
            report(&outposts, &outpost_id, &by_order(&order_id)).await,
            want,
            "case {case:?}: reading the report must not change what a later caller sees"
        );
        answers.push(answer);

        // The scoped report pages like any other, resuming exactly where the page
        // before it stopped.
        for size in [1usize, 3, unscoped.len() + 1] {
            assert_eq!(
                walk(&outposts, &outpost_id, &order_id, size, unscoped.len()).await,
                want,
                "case {case:?}: paging a scoped report {size} at a time must yield it once, \
                 in order"
            );
        }
    }

    // How many units an order bought, the order its line items are named in, an
    // unpublished SKU beside a published configuration and a cancellation are all
    // things the CAPABILITY of the hardware the order is for does not turn on.
    for (case, left, right) in [
        ("several units of one configuration", 0usize, 1usize),
        ("the line items named the other way round", 2, 3),
        ("an unpublished SKU beside a published one", 0, 5),
        ("a cancelled order", 0, 6),
    ] {
        assert_eq!(
            answers[left], answers[right],
            "case {case:?}: expected the same report as the order of one unit of that \
             configuration, got {:?}",
            answers[right]
        );
    }
    // Two configurations at once is a WIDER scope than either of them alone.
    assert!(
        answers[2].len() > answers[0].len()
            && answers[0].iter().all(|entry| answers[2].contains(entry)),
        "an order of two configurations must report at least what an order of one of them \
         does, got {:?} against {:?}",
        answers[2],
        answers[0]
    );
}

/// One row of the asset table: the hardware one asset of the Outpost records, beside
/// the order the report is also scoped to.
struct AssetRow {
    /// What this row is a case of, named in every assertion it makes.
    case: &'static str,
    /// The asset's `AssetId`, a value the modelled `AssetIdInput` (`^\d{10}$`) accepts.
    asset_id: &'static str,
    /// The asset's `AssetType`.
    asset_type: &'static str,
    /// Whether the asset records the first family the order bought capacity of.
    records_ordered_family: bool,
    /// The `MaxVcpus` the asset records, if any.
    max_vcpus: Option<i32>,
    /// Whether the asset is compute hardware at all.
    compute: bool,
}

/// The rule where a request names BOTH scopes: the report is narrowed by the order's
/// published capacity AND by the facts the named asset records, so the answer is the
/// intersection — and an asset that hosts no instance supports none however much was
/// ordered.
#[tokio::test]
async fn an_order_and_an_asset_scope_narrow_together() {
    let outposts = Outposts::new();
    let catalog = catalog();
    let configurations = orderable_configurations(&outposts).await;
    let outpost_id = an_outpost(&outposts, "order-and-asset", Some("RACK")).await;

    // The configuration whose capacity this Outpost's form factor really offers: a
    // report scoped to anything else has nothing to narrow, so the row would prove
    // nothing. Read off the published catalog rather than spelled out.
    let configuration = configurations
        .iter()
        .find(|item| {
            !expected(
                &catalog,
                &Scope::ordered(Some("RACK"), ordered_capacity(&catalog, &[item])),
            )
            .is_empty()
        })
        .expect("a published configuration this Outpost's form factor offers capacity of");
    let ordered = ordered_capacity(&catalog, &[configuration]);
    let order_id = an_order(
        &outposts,
        &outpost_id,
        &[(text(configuration, "CatalogItemId"), 1)],
    )
    .await;
    // One family that order bought capacity of, and a vCPU ceiling at the smallest
    // entry the order-scoped report carries — both read off that report, so neither is
    // a value this test spells out.
    let scoped = expected(&catalog, &Scope::ordered(Some("RACK"), ordered.clone()));
    let families: Vec<String> = vec![
        scoped
            .first()
            .map(|(instance_type, _)| {
                instance_type
                    .split('.')
                    .next()
                    .unwrap_or_default()
                    .to_string()
            })
            .expect("the order-scoped report carries an entry"),
    ];
    let families: Vec<&str> = families.iter().map(String::as_str).collect();
    let ceiling = scoped
        .iter()
        .map(|(_, vcpus)| *vcpus)
        .min()
        .expect("the order-scoped report carries an entry");

    let rows: &[AssetRow] = &[
        AssetRow {
            case: "an asset of a family the order bought, no ceiling",
            asset_id: "4000000001",
            asset_type: "COMPUTE",
            records_ordered_family: true,
            max_vcpus: None,
            compute: true,
        },
        AssetRow {
            case: "an asset that records a vCPU ceiling only",
            asset_id: "4000000002",
            asset_type: "COMPUTE",
            records_ordered_family: false,
            max_vcpus: Some(i32::try_from(ceiling).unwrap_or_default()),
            compute: true,
        },
        AssetRow {
            case: "an asset of that family with that ceiling",
            asset_id: "4000000003",
            asset_type: "COMPUTE",
            records_ordered_family: true,
            max_vcpus: Some(i32::try_from(ceiling).unwrap_or_default()),
            compute: true,
        },
        AssetRow {
            case: "compute hardware that records neither",
            asset_id: "4000000004",
            asset_type: "COMPUTE",
            records_ordered_family: false,
            max_vcpus: None,
            compute: true,
        },
        AssetRow {
            case: "storage hardware, which hosts no instance",
            asset_id: "4000000005",
            asset_type: "STORAGE",
            records_ordered_family: false,
            max_vcpus: None,
            compute: false,
        },
    ];

    let installed: Vec<_> = rows
        .iter()
        .map(|row| {
            asset(
                row.asset_id,
                "rack-01",
                Some(row.asset_type),
                row.compute.then(|| {
                    compute(
                        Some("h-0000000000000001"),
                        Some("ACTIVE"),
                        row.records_ordered_family.then_some(families.as_slice()),
                        None,
                        row.max_vcpus,
                    )
                }),
                None,
            )
        })
        .collect();
    record(&outposts, &outpost_id, &installed, "order and asset");

    for row in rows {
        let case = row.case;
        let want = expected(
            &catalog,
            &Scope {
                form_factor: Some("RACK"),
                ordered: ordered.clone(),
                families: row.records_ordered_family.then_some(families.as_slice()),
                max_vcpus: row.max_vcpus.map(i64::from),
                not_compute: !row.compute,
            },
        );
        let answer = report(
            &outposts,
            &outpost_id,
            &format!(
                "?OrderId={}&AssetId={}",
                query_value(&order_id),
                query_value(row.asset_id)
            ),
        )
        .await;
        assert_eq!(
            answer, want,
            "case {case:?}: expected the order's capacity narrowed by that asset's own \
             hardware, got {answer:?}"
        );
        assert!(
            answer.iter().all(|entry| scoped.contains(entry)),
            "case {case:?}: naming an asset beside the order must narrow, got {answer:?} \
             against {scoped:?}"
        );
    }

    // The rows are not all the same answer: the family scope, the ceiling scope and
    // hardware that hosts nothing each report something of their own.
    let mut reports: Vec<Vec<(String, i64)>> = Vec::new();
    for row in rows {
        reports.push(
            report(
                &outposts,
                &outpost_id,
                &format!(
                    "?OrderId={}&AssetId={}",
                    query_value(&order_id),
                    query_value(row.asset_id)
                ),
            )
            .await,
        );
    }
    assert!(
        reports[4].is_empty(),
        "hardware that hosts no instance supports no instance type however much was \
         ordered, got {:?}",
        reports[4]
    );
    assert!(
        !reports[0].is_empty() && !reports[1].is_empty() && reports[0] != reports[1],
        "a family scope and a ceiling scope must select different hardware, got {:?} and {:?}",
        reports[0],
        reports[1]
    );
    assert_eq!(
        reports[3], scoped,
        "an asset that records neither fact narrows the order's capacity by nothing, got {:?}",
        reports[3]
    );
    // The row where BOTH asset facts meet the order clause: an asset of that family
    // WITH that ceiling is narrower than either fact alone.
    assert!(
        !reports[2].is_empty()
            && reports[2].len() < reports[0].len()
            && reports[2].len() < reports[1].len()
            && reports[2]
                .iter()
                .all(|entry| reports[0].contains(entry) && reports[1].contains(entry)),
        "an asset recording that family AND that ceiling must report what both admit, got \
         {:?} against {:?} and {:?}",
        reports[2],
        reports[0],
        reports[1]
    );
}
