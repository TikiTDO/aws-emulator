// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the HARDWARE SPECIFICATION a published catalog item reports —
//! its class, the current it is fed with, its physical form, its height in rack units
//! and the uplink speeds it supports — driven over the wire through the same front
//! door as every other suite (the harness in [`crate::site_and_outpost_lifecycle`]).
//!
//! The rules under test, each stated once and then exercised over a SET of inputs (the
//! WHOLE published catalog, which spans both modelled item classes and both statuses):
//!
//! 1. Every item reports the specification of the hardware it publishes, and every
//!    member of it follows from that hardware:
//!    * `ItemClass` is the class the item is of, one of the modelled
//!      `CatalogItemClass` values, and it is the fact `ItemClassFilter` selects on, so
//!      a filtered read answers exactly the items reporting that class;
//!    * `RackUnitHeight` is one of the modelled `RackUnitHeight` values, and which one
//!      follows from the class: a RACK is the whole rack, a SERVER one of the
//!      single-unit heights;
//!    * `Dimensions` is the physical form of that hardware, so it follows from the
//!      class and the height: a rack is reported as a rack, a server by the rack units
//!      it occupies;
//!    * `CurrentType` is the electrical current the hardware draws, which every
//!      configuration a site can be prepared for draws (the modelled
//!      `RackPhysicalProperties` power connectors and phases are alternating-current
//!      throughout);
//!    * `SupportedUplinkGiB` and `SupportedUplinkGbps` are the same published fact
//!      about the connection to the Region, so an item reports the same speeds in
//!      both.
//! 2. `EC2Capacities` splits an instance type between its two members: `Family` names
//!    the family and `MaxSize` the SIZE SUFFIX alone, so neither repeats the other and
//!    the two join into an instance type `ListOrderableInstanceTypes` publishes.
//! 3. The single-item read answers the same specification as the listing, member for
//!    member, for every SKU the catalog publishes — a client that read a SKU out of
//!    the list reads the same hardware facts back by it.
//!
//! What a catalog item reports about its power, weight, storage and scaling, and how
//! the catalog is filtered and paged, is [`crate::catalog_items`]; the single-item read
//! itself is [`crate::get_catalog_item`].

use std::collections::BTreeSet;

use serde_json::Value;

use crate::site_and_outpost_lifecycle::{segment, text, Outposts};

/// The modelled `CatalogItemClass` values (`RACK` | `SERVER`), as
/// `smithy_query(query="shape", shape="CatalogItemClass")` reports them.
const ITEM_CLASSES: &[&str] = &["RACK", "SERVER"];

/// The modelled `RackUnitHeight` values, as
/// `smithy_query(query="shape", shape="RackUnitHeight")` reports them (`HEIGHT_42U` |
/// `HEIGHT_2U` | `HEIGHT_1U`).
const RACK_UNIT_HEIGHTS: &[&str] = &["HEIGHT_42U", "HEIGHT_2U", "HEIGHT_1U"];

/// The height of a whole rack, from that same value set: the one value of it that is
/// not a single-unit height.
const RACK_HEIGHT: &str = "HEIGHT_42U";

/// The `Dimensions` of a whole rack.
const RACK_DIMENSIONS: &str = "OUTPOST_RACK";

/// The electrical current the hardware of every published configuration is fed with.
///
/// Source: the power a SITE declares it can provide at a hardware placement position,
/// which is the modelled `RackPhysicalProperties` — its `PowerConnector` values
/// (`AH530P7W`, `AH532P6W`, `IEC309`, `L6_30P`) and its `PowerPhase` values
/// (`SINGLE_PHASE` | `THREE_PHASE`) are alternating-current connectors and phases
/// throughout.
const CURRENT_TYPE: &str = "AC";

/// The `CatalogItems` of one `ListCatalogItems` answer.
fn items(output: &Value) -> Vec<Value> {
    output
        .get("CatalogItems")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The whole published catalog, in the order `ListCatalogItems` reports it — the page
/// size is the modelled `MaxResults` maximum, so one call exhausts it.
async fn catalog(outposts: &Outposts) -> Vec<Value> {
    items(
        &outposts
            .ok("GET", "/catalog/items?MaxResults=1000", None)
            .await,
    )
}

/// The catalog narrowed to one modelled `CatalogItemClass`.
async fn catalog_of_class(outposts: &Outposts, class: &str) -> Vec<Value> {
    items(
        &outposts
            .ok(
                "GET",
                &format!("/catalog/items?MaxResults=1000&ItemClassFilter={class}"),
                None,
            )
            .await,
    )
}

/// Every instance type this service holds a specification of.
///
/// The wider of its two instance-type lists: a configuration's published capacity
/// names the whole host of a family its hardware runs, including a family offered on
/// SERVER hardware alone, which the catalog `ListOrderableInstanceTypes` publishes no
/// entry for (see [`crate::instance_type_catalogs`]).
fn specified_instance_types() -> BTreeSet<String> {
    crate::instance_type_catalogs::specifications()
        .iter()
        .map(|entry| text(entry, "InstanceType"))
        .collect()
}

/// A reported list-of-integers member, empty when the wire omits it.
fn integers(item: &Value, key: &str) -> Vec<i64> {
    item.get(key)
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default()
}

/// The `Dimensions` the hardware of `class` and `rack_unit_height` takes: a rack is a
/// whole rack, and a server is the rack units it occupies in the customer's own rack —
/// the same fact its `RackUnitHeight` reports.
fn dimensions_of(class: &str, rack_unit_height: &str) -> String {
    match class {
        "RACK" => RACK_DIMENSIONS.to_string(),
        _ => format!(
            "OUTPOST_SERVER_{}",
            rack_unit_height.trim_start_matches("HEIGHT_")
        ),
    }
}

/// Rule 1: every published item reports the specification of its own hardware.
#[tokio::test]
async fn every_catalog_item_reports_the_specification_of_its_hardware() {
    let outposts = Outposts::new();
    let catalog = catalog(&outposts).await;
    assert!(
        catalog.len() > 1,
        "the service publishes a catalog of configurations, got {catalog:?}"
    );

    let mut classes: BTreeSet<String> = BTreeSet::new();
    let mut heights: BTreeSet<String> = BTreeSet::new();
    for item in &catalog {
        let case = text(item, "CatalogItemId");
        let class = text(item, "ItemClass");
        assert!(
            ITEM_CLASSES.contains(&class.as_str()),
            "case {case:?}: ItemClass must be a modelled CatalogItemClass, got {item:?}"
        );
        let height = text(item, "RackUnitHeight");
        assert!(
            RACK_UNIT_HEIGHTS.contains(&height.as_str()),
            "case {case:?}: RackUnitHeight must be a modelled RackUnitHeight, got {item:?}"
        );
        assert_eq!(
            height == RACK_HEIGHT,
            class == "RACK",
            "case {case:?}: the whole-rack height is exactly what a RACK reports, got \
             {item:?}"
        );
        assert_eq!(
            text(item, "Dimensions"),
            dimensions_of(&class, &height),
            "case {case:?}: Dimensions must be the form the class and the height name, got \
             {item:?}"
        );
        assert_eq!(
            text(item, "CurrentType"),
            CURRENT_TYPE,
            "case {case:?}: CurrentType must be the current the hardware draws, got {item:?}"
        );
        let uplink_gbps = integers(item, "SupportedUplinkGbps");
        assert!(
            !uplink_gbps.is_empty(),
            "case {case:?}: an item names the uplink speeds it supports, got {item:?}"
        );
        assert_eq!(
            integers(item, "SupportedUplinkGiB"),
            uplink_gbps,
            "case {case:?}: both uplink members report the same published speeds, got {item:?}"
        );
        classes.insert(class);
        heights.insert(height);
    }

    // The catalog spans what the rule generalises over, so no clause above passed for
    // want of a case: both modelled classes, and a single-unit height beside the
    // whole-rack one.
    assert_eq!(
        classes.iter().map(String::as_str).collect::<BTreeSet<_>>(),
        ITEM_CLASSES.iter().copied().collect::<BTreeSet<_>>(),
        "the catalog must publish an item of every modelled class, got {classes:?}"
    );
    assert!(
        heights.len() > 1 && heights.contains(RACK_HEIGHT),
        "the catalog must publish more than one rack unit height, got {heights:?}"
    );
}

/// Rule 1's filter clause: `ItemClassFilter` selects on the class the item reports of
/// itself, over every modelled class.
#[tokio::test]
async fn the_class_filter_selects_on_the_class_an_item_reports() {
    let outposts = Outposts::new();
    let catalog = catalog(&outposts).await;

    for class in ITEM_CLASSES {
        let expected: Vec<Value> = catalog
            .iter()
            .filter(|item| text(item, "ItemClass") == *class)
            .cloned()
            .collect();
        assert!(
            !expected.is_empty(),
            "case {class:?}: the catalog must publish something of every modelled class"
        );
        assert_eq!(
            catalog_of_class(&outposts, class).await,
            expected,
            "case {class:?}: the filter must select exactly the items reporting that class, \
             and report each one whole"
        );
    }
}

/// Rule 2: `Family` and `MaxSize` split an instance type between them, over every EC2
/// capacity of every published item.
#[tokio::test]
async fn an_ec2_capacity_names_its_family_and_the_bare_size_of_it() {
    let outposts = Outposts::new();
    let catalog = catalog(&outposts).await;
    let published = specified_instance_types();
    let mut checked = 0usize;

    for item in &catalog {
        let case = text(item, "CatalogItemId");
        let capacities = item
            .get("EC2Capacities")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            !capacities.is_empty(),
            "case {case:?}: a configuration carries EC2 capacity, got {item:?}"
        );
        for capacity in &capacities {
            let family = text(capacity, "Family");
            let max_size = text(capacity, "MaxSize");
            assert!(
                !max_size.is_empty() && !max_size.contains('.'),
                "case {case:?}: MaxSize must be the bare size suffix, got {capacity:?}"
            );
            assert!(
                !family.is_empty() && !family.contains('.'),
                "case {case:?}: Family must be the bare family prefix, got {capacity:?}"
            );
            let instance_type = format!("{family}.{max_size}");
            assert!(
                published.contains(&instance_type),
                "case {case:?}: Family and MaxSize must join into an instance type this \
                 service holds a specification of, got {instance_type:?} for {capacity:?}"
            );
            checked += 1;
        }
    }
    assert!(
        checked > ITEM_CLASSES.len(),
        "the catalog must publish capacities to check, got {checked}"
    );
}

/// Rule 3: the single-item read answers the same specification as the listing, for
/// EVERY SKU the catalog publishes.
#[tokio::test]
async fn the_single_item_read_answers_the_same_specification() {
    let outposts = Outposts::new();
    let catalog = catalog(&outposts).await;

    for item in &catalog {
        let sku = text(item, "CatalogItemId");
        let read = outposts
            .ok("GET", &format!("/catalog/item/{}", segment(&sku)), None)
            .await["CatalogItem"]
            .clone();
        assert_eq!(
            &read, item,
            "case {sku:?}: the single-item read must answer the item the listing reports, \
             member for member"
        );
        for member in [
            "ItemClass",
            "CurrentType",
            "Dimensions",
            "RackUnitHeight",
            "SupportedUplinkGiB",
        ] {
            assert!(
                read.get(member).is_some(),
                "case {sku:?}: the read must report {member}, got {read:?}"
            );
        }
    }
}
