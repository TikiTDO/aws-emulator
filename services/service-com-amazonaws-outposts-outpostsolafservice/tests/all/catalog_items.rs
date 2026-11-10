// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListCatalogItems`, driven over the wire through the same
//! front door as every other suite (the harness in
//! [`crate::site_and_outpost_lifecycle`]).
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. The operation reports the CATALOG of orderable hardware configurations the
//!    service publishes: every item carries a `CatalogItemId` of the modelled
//!    `SkuCode` form, a modelled `CatalogItemStatus`, a positive power draw and
//!    weight, at least one modelled uplink speed, at least one modelled storage
//!    option, a `RackScalingType` that is modelled whenever it is reported, and EC2
//!    capacity whose every entry names an instance family, the largest instance type
//!    of that family the hardware runs — one `ListOrderableInstanceTypes` publishes
//!    for the same hardware — and a whole, positive quantity of it.
//! 2. `ItemClassFilter` partitions the catalog: every item has exactly one class, so
//!    the two single-class answers are disjoint, together they are the whole catalog,
//!    naming both classes selects the whole catalog, and naming one twice selects
//!    what naming it once does.
//! 3. `SupportedStorageFilter` and `EC2FamilyFilter` each select exactly the items
//!    ANY of their values matches, over a table of values drawn from the catalog
//!    itself plus values nothing matches, and report each selected item WHOLE.
//! 4. Filters given together narrow rather than widen: an answer under several
//!    filters is exactly the intersection of the answers under each of them, in
//!    catalog order.
//! 5. The catalog is cut into pages of at most `MaxResults` with a `NextToken` that
//!    resumes exactly where the previous page stopped, for any page size and with or
//!    without filters; the last page carries no token; and reading the catalog
//!    changes nothing a later caller sees.
//! 6. A `NextToken` this service did not issue for THIS list is a bad request (400
//!    `ValidationException`), as is a `MaxResults` outside the modelled range and a
//!    filter value outside the modelled value set; a boundary past the end of the
//!    catalog is an empty last page, not a refusal.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::site_and_outpost_lifecycle::{query_value, text, Outposts};

/// The modelled `CatalogItemClass` values (`RACK` | `SERVER`), as
/// `smithy_query(query="shape", shape="CatalogItemClass")` reports them.
const ITEM_CLASSES: &[&str] = &["RACK", "SERVER"];

/// The modelled `CatalogItemStatus` values, from the same model.
const ITEM_STATUSES: &[&str] = &["AVAILABLE", "DISCONTINUED"];

/// The modelled `SupportedStorageEnum` values, from the same model.
const STORAGE_OPTIONS: &[&str] = &["EBS", "S3"];

/// The modelled `RackScalingType` values, which `CatalogItem$RackScalingType`
/// documents as its valid values.
const RACK_SCALING_TYPES: &[&str] = &["SINGLE_RACK", "MULTI_RACK"];

/// The uplink speeds, in Gbps, the modelled `UplinkGbps` enum names (`UPLINK_1G` |
/// `UPLINK_10G` | `UPLINK_40G` | `UPLINK_100G`) — the set a reported
/// `SupportedUplinkGbps` integer may come from.
const UPLINK_GBPS: &[i64] = &[1, 10, 40, 100];

/// An instance family that is a well-formed `Family` (the modelled pattern
/// `^[a-z0-9]+[a-z0-9-]*[a-z0-9]+$`) but that no published configuration carries
/// capacity of, so a filter naming it selects nothing.
const ABSENT_FAMILY: &str = "x2iedn";

/// Whether `id` is the modelled `SkuCode` form `^OR-[A-Z0-9]{7}$`.
fn is_sku_code(id: &str) -> bool {
    match id.strip_prefix("OR-") {
        Some(body) => {
            body.len() == 7
                && body
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        }
        None => false,
    }
}

/// Whether `family` is a well-formed modelled `Family` (len 1..10, pattern
/// `^[a-z0-9]+[a-z0-9-]*[a-z0-9]+$`).
fn is_family(family: &str) -> bool {
    !family.is_empty()
        && family.len() <= 10
        && family
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !family.starts_with('-')
        && !family.ends_with('-')
}

/// The `CatalogItems` list of one answer.
fn items(output: &Value) -> Vec<Value> {
    output
        .get("CatalogItems")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `CatalogItemId` of every item of one answer, in the order it reported them.
fn ids(output: &Value) -> Vec<String> {
    items(output)
        .iter()
        .map(|item| text(item, "CatalogItemId"))
        .collect()
}

/// The storage options one item reports.
fn storage_of(item: &Value) -> Vec<String> {
    strings(item, "SupportedStorage")
}

/// A reported list-of-strings member, empty when the wire omits it.
fn strings(item: &Value, key: &str) -> Vec<String> {
    item.get(key)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The EC2 capacity entries one item reports.
fn capacities(item: &Value) -> Vec<Value> {
    item.get("EC2Capacities")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The instance families one item carries capacity of.
fn families_of(item: &Value) -> Vec<String> {
    capacities(item)
        .iter()
        .map(|capacity| text(capacity, "Family"))
        .collect()
}

/// The three `*Filter` members of one request, each a list of accepted values for
/// ONE fact about an item. A list left empty is one the request never carried.
#[derive(Debug, Default, Clone)]
struct Filters {
    classes: Vec<String>,
    storage: Vec<String>,
    families: Vec<String>,
}

impl Filters {
    /// The query string these filters travel in: each modelled `@httpQuery` list
    /// member is repeated once per accepted value, which is what an AWS SDK puts on
    /// the wire.
    fn query(&self) -> String {
        let mut params: Vec<String> = Vec::new();
        for (name, values) in [
            ("ItemClassFilter", &self.classes),
            ("SupportedStorageFilter", &self.storage),
            ("EC2FamilyFilter", &self.families),
        ] {
            for value in values {
                params.push(format!("{name}={}", query_value(value)));
            }
        }
        match params.is_empty() {
            true => String::new(),
            false => format!("?{}", params.join("&")),
        }
    }

    /// The same parameters, without the leading `?`, for a query that carries more.
    fn params(&self) -> String {
        self.query().replacen('?', "", 1)
    }
}

/// The accepted values of one filter, as the strings a request carries.
fn values(accepted: &[&str]) -> Vec<String> {
    accepted.iter().map(|value| (*value).to_string()).collect()
}

/// `ListCatalogItems` with the given query string, requiring HTTP 200.
async fn list(outposts: &Outposts, query: &str) -> Value {
    outposts.ok("GET", &format!("/catalog/items{query}"), None).await
}

/// `ListCatalogItems` under `filters`, requiring HTTP 200.
async fn filtered(outposts: &Outposts, filters: &Filters) -> Value {
    list(outposts, &filters.query()).await
}

/// Walk the catalog with `MaxResults=size`, following every `NextToken` to the end,
/// and answer the item ids the walk collected.
///
/// `extra` is whatever else the query carries (filters, or nothing).
async fn walk(outposts: &Outposts, size: usize, extra: &str, ceiling: usize) -> Vec<String> {
    let mut collected: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let query = match &token {
            Some(token) => format!("?MaxResults={size}{extra}&NextToken={}", query_value(token)),
            None => format!("?MaxResults={size}{extra}"),
        };
        let page = list(outposts, &query).await;
        let page_ids = ids(&page);
        assert!(
            page_ids.len() <= size,
            "case size {size}{extra}: a page holds at most MaxResults items, got {} items",
            page_ids.len()
        );
        collected.extend(page_ids);
        pages += 1;
        assert!(
            pages <= ceiling + 1,
            "case size {size}{extra}: paging must terminate, got {pages} pages and {collected:?}"
        );
        match page.get("NextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    collected
}

/// Every instance type this service holds a specification of, as `(instance type,
/// VCPUs)` — the table an item's EC2 capacity has to name a member of.
///
/// The wider of the service's two instance-type lists: a SERVER configuration carries
/// capacity of a family offered on server hardware alone, which the catalog
/// `ListOrderableInstanceTypes` publishes no entry for (see
/// [`crate::instance_type_catalogs`]).
fn specified_instance_types() -> Vec<(String, i64)> {
    crate::instance_type_catalogs::specifications()
        .iter()
        .map(|entry| {
            (
                text(entry, "InstanceType"),
                entry.get("VCPUs").and_then(Value::as_i64).unwrap_or_default(),
            )
        })
        .collect()
}

/// Rule 1: every published item reports modelled, internally consistent hardware —
/// checked over the WHOLE catalog, not one row, and cross-checked against the
/// instance-type catalog the same service publishes.
#[tokio::test]
async fn every_catalog_item_reports_modelled_hardware() {
    let outposts = Outposts::new();
    let answer = list(&outposts, "").await;
    let catalog = items(&answer);
    let instance_types = specified_instance_types();

    assert!(
        catalog.len() > 1,
        "the service publishes a catalog of configurations, got {catalog:?}"
    );
    assert!(
        answer.get("NextToken").is_none(),
        "a page that exhausts the catalog issues no NextToken, got {answer:?}"
    );

    for item in &catalog {
        let case = text(item, "CatalogItemId");
        assert!(
            is_sku_code(&case),
            "case {case:?}: CatalogItemId must be a modelled SkuCode, got {item:?}"
        );
        let status = text(item, "ItemStatus");
        assert!(
            ITEM_STATUSES.contains(&status.as_str()),
            "case {case:?}: ItemStatus must be modelled, got {status:?}"
        );
        let power = item.get("PowerKva").and_then(Value::as_f64);
        assert!(
            power.is_some_and(|power| power > 0.0),
            "case {case:?}: PowerKva must be the power the item draws, got {item:?}"
        );
        let weight = item.get("WeightLbs").and_then(Value::as_i64);
        assert!(
            weight.is_some_and(|weight| weight > 0),
            "case {case:?}: WeightLbs must be what the item weighs, got {item:?}"
        );

        let uplinks: Vec<i64> = item
            .get("SupportedUplinkGbps")
            .and_then(Value::as_array)
            .map(|values| values.iter().filter_map(Value::as_i64).collect())
            .unwrap_or_default();
        assert!(
            !uplinks.is_empty() && uplinks.iter().all(|speed| UPLINK_GBPS.contains(speed)),
            "case {case:?}: SupportedUplinkGbps must name modelled uplink speeds, got {item:?}"
        );
        let storage = storage_of(item);
        assert!(
            !storage.is_empty()
                && storage
                    .iter()
                    .all(|option| STORAGE_OPTIONS.contains(&option.as_str())),
            "case {case:?}: SupportedStorage must name modelled storage options, got {item:?}"
        );
        if let Some(scaling) = item.get("RackScalingType").and_then(Value::as_str) {
            assert!(
                RACK_SCALING_TYPES.contains(&scaling),
                "case {case:?}: RackScalingType must be modelled, got {scaling:?}"
            );
        }

        let capacity = capacities(item);
        assert!(
            !capacity.is_empty(),
            "case {case:?}: a configuration carries EC2 capacity, got {item:?}"
        );
        for entry in &capacity {
            let family = text(entry, "Family");
            let max_size = text(entry, "MaxSize");
            let quantity = text(entry, "Quantity");
            assert!(
                is_family(&family),
                "case {case:?}: {family:?} must be a modelled Family, got {entry:?}"
            );
            // `Family` names the family and `MaxSize` the SIZE alone, so neither
            // repeats the other and the instance type is the join of the two.
            assert!(
                !max_size.contains('.'),
                "case {case:?}: MaxSize must be the bare size suffix, not a whole instance \
                 type, got {entry:?}"
            );
            let instance_type = format!("{family}.{max_size}");
            let vcpus = instance_types
                .iter()
                .find(|(published, _)| *published == instance_type)
                .map(|(_, vcpus)| *vcpus);
            assert!(
                vcpus.is_some_and(|vcpus| vcpus > 0),
                "case {case:?}: Family and MaxSize must join into {instance_type:?}, an \
                 instance type this service publishes as orderable, got {entry:?}"
            );
            assert!(
                quantity.parse::<i64>().is_ok_and(|quantity| quantity >= 1),
                "case {case:?}: Quantity must be a whole, positive count, got {entry:?}"
            );
            assert_eq!(
                capacity
                    .iter()
                    .filter(|other| text(other, "Family") == family)
                    .count(),
                1,
                "case {case:?}: a family's capacity is reported once, got {item:?}"
            );
        }
    }

    // The catalog is a set, reported in one stable order: every id is published once,
    // and the order does not depend on who asks.
    let reported = ids(&answer);
    assert_eq!(
        reported.iter().collect::<BTreeSet<_>>().len(),
        reported.len(),
        "no catalog item is published twice, got {reported:?}"
    );
    let mut sorted = reported.clone();
    sorted.sort();
    assert_eq!(
        reported, sorted,
        "the catalog is reported in one stable (SkuCode) order"
    );
    assert_eq!(
        ids(&list(&outposts, "").await),
        reported,
        "the same call answers the same catalog every time"
    );
}

/// Rule 2: the class filter partitions the catalog over every modelled class.
#[tokio::test]
async fn the_class_filter_partitions_the_catalog() {
    let outposts = Outposts::new();
    let whole = ids(&list(&outposts, "").await);

    let mut union: Vec<String> = Vec::new();
    for class in ITEM_CLASSES {
        let selected = ids(&filtered(
            &outposts,
            &Filters {
                classes: values(&[class]),
                ..Filters::default()
            },
        )
        .await);
        assert!(
            !selected.is_empty(),
            "case {class:?}: the catalog must publish something of every modelled class"
        );
        assert!(
            selected.iter().all(|id| whole.contains(id)),
            "case {class:?}: a filtered answer holds only published items, got {selected:?}"
        );
        // Naming one value twice is naming it: a filter is a SET of accepted values.
        assert_eq!(
            ids(&filtered(
                &outposts,
                &Filters {
                    classes: values(&[class, class]),
                    ..Filters::default()
                },
            )
            .await),
            selected,
            "case {class:?}: naming a class twice selects what naming it once does"
        );
        union.extend(selected);
    }

    assert_eq!(
        union.iter().collect::<BTreeSet<_>>().len(),
        union.len(),
        "every item has exactly ONE class, so the per-class answers are disjoint, got {union:?}"
    );
    union.sort();
    let mut expected = whole.clone();
    expected.sort();
    assert_eq!(
        union, expected,
        "every published item is of some modelled class, so the classes cover the catalog"
    );

    // Naming every class selects the whole catalog, in catalog order.
    assert_eq!(
        ids(&filtered(
            &outposts,
            &Filters {
                classes: values(ITEM_CLASSES),
                ..Filters::default()
            },
        )
        .await),
        whole,
        "naming every modelled class selects the whole catalog"
    );

    // A SERVER is not a rack, so rack scaling does not apply to one: the member is
    // omitted rather than carrying a value the hardware has no answer for.
    for item in items(&filtered(
        &outposts,
        &Filters {
            classes: values(&["SERVER"]),
            ..Filters::default()
        },
    )
    .await)
    {
        assert!(
            item.get("RackScalingType").is_none(),
            "case {:?}: a server reports no RackScalingType, got {item:?}",
            text(&item, "CatalogItemId")
        );
    }
}

/// Rule 3: the storage and family filters each select the items any of their values
/// matches, over a table of values drawn from the catalog itself — including a value
/// nothing matches — with every expectation COMPUTED from the rule.
#[tokio::test]
async fn storage_and_family_filters_select_items_matching_any_value() {
    let outposts = Outposts::new();
    let catalog = items(&list(&outposts, "").await);

    // Every family the catalog carries capacity of, so no published family goes
    // untested, plus a well-formed family nothing carries.
    let published: BTreeSet<String> = catalog.iter().flat_map(families_of).collect();
    assert!(
        published.len() > 1,
        "the catalog must carry capacity of several families, got {published:?}"
    );

    let mut cases: Vec<(String, Filters)> = vec![
        ("no filter at all".to_string(), Filters::default()),
        (
            "every storage option at once".to_string(),
            Filters {
                storage: values(STORAGE_OPTIONS),
                ..Filters::default()
            },
        ),
        (
            "a family nothing carries".to_string(),
            Filters {
                families: values(&[ABSENT_FAMILY]),
                ..Filters::default()
            },
        ),
        (
            "a published family and one nothing carries".to_string(),
            Filters {
                families: vec![
                    published.iter().next_back().cloned().unwrap_or_default(),
                    ABSENT_FAMILY.to_string(),
                ],
                ..Filters::default()
            },
        ),
        (
            "every published family at once".to_string(),
            Filters {
                families: published.iter().cloned().collect(),
                ..Filters::default()
            },
        ),
    ];
    for option in STORAGE_OPTIONS {
        cases.push((
            format!("storage {option:?}"),
            Filters {
                storage: values(&[option]),
                ..Filters::default()
            },
        ));
    }
    for family in &published {
        cases.push((
            format!("family {family:?}"),
            Filters {
                families: vec![family.clone()],
                ..Filters::default()
            },
        ));
    }

    for (case, filters) in &cases {
        // The rule, applied to the whole catalog: an item is selected when every
        // filter the request carried accepts it, and a filter accepts it when any one
        // of its values matches.
        let expected: Vec<Value> = catalog
            .iter()
            .filter(|item| {
                let accepts = |filter: &Vec<String>, reported: Vec<String>| match filter.is_empty()
                {
                    true => true,
                    false => reported.iter().any(|value| filter.contains(value)),
                };
                accepts(&filters.storage, storage_of(item))
                    && accepts(&filters.families, families_of(item))
            })
            .cloned()
            .collect();
        let answered = items(&filtered(&outposts, filters).await);
        assert_eq!(
            answered, expected,
            "case {case:?}: the filters must select exactly the matching items, and report \
             each one whole, got {answered:?}"
        );
    }
}

/// Rule 4: filters given together narrow — an answer under several filters is the
/// intersection of the answers under each one alone, in catalog order.
#[tokio::test]
async fn filters_given_together_narrow_to_their_intersection() {
    let outposts = Outposts::new();
    let whole = ids(&list(&outposts, "").await);
    let catalog = items(&list(&outposts, "").await);
    let a_family = catalog
        .iter()
        .flat_map(families_of)
        .next()
        .unwrap_or_default();
    let another_family = catalog
        .iter()
        .flat_map(families_of)
        .next_back()
        .unwrap_or_default();

    let cases: Vec<(&str, Filters)> = vec![
        (
            "class and storage",
            Filters {
                classes: values(&["RACK"]),
                storage: values(&["S3"]),
                ..Filters::default()
            },
        ),
        (
            "class and family",
            Filters {
                classes: values(&["SERVER"]),
                families: vec![a_family.clone()],
                ..Filters::default()
            },
        ),
        (
            "storage and family",
            Filters {
                storage: values(&["EBS"]),
                families: vec![another_family.clone()],
                ..Filters::default()
            },
        ),
        (
            "all three filters at once",
            Filters {
                classes: values(&["RACK"]),
                storage: values(&["EBS", "S3"]),
                families: vec![a_family, another_family],
            },
        ),
        (
            "a combination nothing satisfies",
            Filters {
                classes: values(&["SERVER"]),
                storage: values(&["S3"]),
                families: vec![ABSENT_FAMILY.to_string()],
            },
        ),
    ];

    for (case, filters) in &cases {
        let mut expected = whole.clone();
        for single in [
            Filters {
                classes: filters.classes.clone(),
                ..Filters::default()
            },
            Filters {
                storage: filters.storage.clone(),
                ..Filters::default()
            },
            Filters {
                families: filters.families.clone(),
                ..Filters::default()
            },
        ] {
            let selected = ids(&filtered(&outposts, &single).await);
            expected.retain(|id| selected.contains(id));
        }
        assert_eq!(
            ids(&filtered(&outposts, filters).await),
            expected,
            "case {case:?}: filters together must narrow to their intersection"
        );
    }
}

/// Rule 5: for EVERY page size, and with and without filters, paging from the first
/// `NextToken` to the last yields exactly what one unpaged call reports, once each,
/// in the same order — and the unpaged call answers the same thing after the whole
/// walk as before it.
#[tokio::test]
async fn paging_the_catalog_covers_every_item_exactly_once() {
    let outposts = Outposts::new();
    let before = list(&outposts, "").await;
    let whole = ids(&before);
    let count = whole.len();

    let filters: Vec<(&str, Filters)> = vec![
        ("no filter", Filters::default()),
        (
            "one class",
            Filters {
                classes: values(&["RACK"]),
                ..Filters::default()
            },
        ),
        (
            "one storage option",
            Filters {
                storage: values(&["S3"]),
                ..Filters::default()
            },
        ),
    ];

    // One per page, sizes that leave a short last page, the count itself and more
    // than the count — so a token that skipped or duplicated an item at any boundary
    // would show up.
    for size in [1usize, 2, 3, count, count + 5] {
        for (case, filter) in &filters {
            let expected = ids(&filtered(&outposts, filter).await);
            let params = filter.params();
            let extra = match params.is_empty() {
                true => String::new(),
                false => format!("&{params}"),
            };
            assert_eq!(
                walk(&outposts, size, &extra, count).await,
                expected,
                "case {case:?} size {size}: paging must yield the whole answer once, in order"
            );
        }
    }

    assert_eq!(
        list(&outposts, "").await,
        before,
        "reading the catalog must not change what a later caller sees"
    );
}

/// Rule 6: what a pagination token, a page size and a filter value may be.
#[tokio::test]
async fn a_token_this_list_did_not_issue_is_refused() {
    let outposts = Outposts::new();
    let count = ids(&list(&outposts, "").await).len();

    // The token this list DOES issue is honoured, and resumes after the first page.
    let first = list(&outposts, "?MaxResults=1").await;
    let token = first
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !token.is_empty(),
        "a catalog of {count} items served one at a time issues a NextToken, got {first:?}"
    );
    let second = list(
        &outposts,
        &format!("?MaxResults=1&NextToken={}", query_value(&token)),
    )
    .await;
    assert_ne!(
        ids(&second),
        ids(&first),
        "the token must resume AFTER the page that issued it, got {second:?}"
    );

    // A boundary past the end of the catalog is an empty last page.
    let past_the_end = list(
        &outposts,
        &format!(
            "?NextToken={}",
            query_value(&format!("{}##catalogItems", count + 1))
        ),
    )
    .await;
    assert_eq!(
        (ids(&past_the_end).len(), past_the_end.get("NextToken")),
        (0, None),
        "a boundary past the end of the catalog is an empty last page, got {past_the_end:?}"
    );

    // Everything that is not a token for THIS list, every page size outside the
    // modelled `MaxResults1000` range, and every filter value outside the value set
    // the model declares for that filter.
    let cases: Vec<(&str, String)> = vec![
        (
            "a token stamped for ListOrderableInstanceTypes",
            "/catalog/items?NextToken=1%23%23instanceTypes".to_string(),
        ),
        (
            "a token stamped for ListOutposts",
            "/catalog/items?NextToken=1%23%23outposts".to_string(),
        ),
        (
            "a well-formed token with a nonsense boundary",
            "/catalog/items?NextToken=abc%23%23catalogItems".to_string(),
        ),
        (
            "a page size below the modelled minimum",
            "/catalog/items?MaxResults=0".to_string(),
        ),
        (
            "a page size above the modelled maximum",
            "/catalog/items?MaxResults=1001".to_string(),
        ),
        (
            "an item class outside the modelled enum",
            "/catalog/items?ItemClassFilter=RACKS".to_string(),
        ),
        (
            "a storage option outside the modelled enum",
            "/catalog/items?SupportedStorageFilter=NVME".to_string(),
        ),
        (
            "one modelled class and one that is not",
            "/catalog/items?ItemClassFilter=RACK&ItemClassFilter=TRUCK".to_string(),
        ),
    ];
    for (case, path) in &cases {
        let reply = outposts.call("GET", path, None).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} {} {:?}",
            reply.status,
            reply.error_code,
            reply.body
        );
    }

    // The other direction: this list's token is not a boundary of another list.
    let replayed = outposts
        .call(
            "GET",
            &format!("/instanceTypes?NextToken={}", query_value(&token)),
            None,
        )
        .await;
    assert_eq!(
        (replayed.status, replayed.error_code.as_str()),
        (400, "ValidationException"),
        "a ListCatalogItems token replayed on ListOrderableInstanceTypes is refused, got {:?}",
        replayed.body
    );
}
