// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the hardware an Outpost is INSTALLED with, driven over the
//! wire through the same front door as [`crate::site_and_outpost_lifecycle`] (whose
//! harness this suite reuses).
//!
//! Where [`crate::outpost_assets`] states what `ListAssets` answers ABOUT the hardware
//! an Outpost holds, this suite states where that hardware comes from. The rules under
//! test, each stated once and then exercised over a SET of inputs:
//!
//! 1. Hardware is what an Outpost IS: Amazon Web Services installs the configuration
//!    the Outpost's form factor names once an order placed on it has delivered
//!    hardware to its site, and the Outpost reaches its settled lifecycle status with
//!    that hardware in it — so an Outpost nothing has been ordered for holds no
//!    asset, and one whose order has been installed holds one `AssetInfo` per physical
//!    unit of its configuration. An Outpost whose form factor names no configuration
//!    this service holds is installed with nothing.
//! 2. Each installed asset reports what that unit is and where it is: an `AssetId` of
//!    the ten digits the modelled `AssetIdInput` takes, its `AssetType` from the
//!    modelled enum, the `RackId` of the one rack a rack configuration's units share,
//!    an `AssetLocation.RackElevation` inside the modelled `range(0..99)`, and —
//!    exactly for the compute units, the only ones the model carries
//!    `ComputeAttributes` for — a Dedicated Host, the `ACTIVE` compute state, and the
//!    instance families and vCPU ceiling `ListOrderableInstanceTypes` publishes as
//!    orderable on that form factor.
//! 3. Hardware is installed ONCE and belongs to ONE Outpost: every later read answers
//!    the same assets, and no two Outposts of a store hold one asset id.
//! 4. An asset id a caller reads out of `ListAssets` is one it can then NAME:
//!    `GetOutpostSupportedInstanceTypes` scoped to it answers the instance types that
//!    unit runs (none for a unit that is not compute hardware), and `StartCapacityTask`
//!    accepts it as the asset the task is for.
//! 5. Because an Outpost holds hardware, the answer's filters and page bound are
//!    observable over the wire: each `*Filter` narrows to the units whose value for
//!    that member it names, and a page holds at most `MaxResults` assets with a
//!    `NextToken` that resumes exactly where it stopped.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::ops::types::ASSET_TYPE_VALUES;

use crate::capacity_task_lifecycle::capacity_route;
use crate::site_and_outpost_lifecycle::{
    install_hardware, orderable_site, query_value, segment, text, Outposts,
};

/// The route `ListAssets` binds, for one identifier form and one query string.
fn assets_route(identifier: &str, query: &str) -> String {
    format!("/outposts/{}/assets{query}", segment(identifier))
}

/// The route `GetOutpostSupportedInstanceTypes` binds.
fn supported_route(identifier: &str, query: &str) -> String {
    format!(
        "/outposts/{}/supportedInstanceTypes{query}",
        segment(identifier)
    )
}

/// The three hardware types an Outpost in this store can carry: both values of the
/// modelled `SupportedHardwareType` enum (`smithy_query(query="shape",
/// shape="SupportedHardwareType")`) and the member omitted.
const HARDWARE_TYPES: &[Option<&str>] = &[Some("RACK"), Some("SERVER"), None];

/// The modelled `ComputeAssetState` installed, usable compute hardware carries.
const COMPUTE_STATE_ACTIVE: &str = "ACTIVE";

/// The published minimum of the modelled `RackElevation` (`range(min: 0, max: 99)`).
const RACK_ELEVATION_MIN: f64 = 0.0;

/// The published maximum of the same range.
const RACK_ELEVATION_MAX: f64 = 99.0;

/// One site and one Outpost on it carrying `form_factor`, as that Outpost's
/// `OutpostId`. An Outpost belongs to a site, so the site comes first.
async fn an_outpost(outposts: &Outposts, name: &str, form_factor: Option<&str>) -> String {
    let site = outposts
        .create_site(&orderable_site(&format!("{name}-site")))
        .await;
    let mut request = json!({ "Name": name, "SiteId": text(&site, "SiteId") });
    if let Some(form_factor) = form_factor {
        request["SupportedHardwareType"] = json!(form_factor);
    }
    let outpost = outposts.create_outpost(&request).await;
    text(&outpost, "OutpostId")
}

/// `ListAssets` on one Outpost, requiring HTTP 200.
async fn list(outposts: &Outposts, identifier: &str, query: &str) -> Value {
    outposts.ok("GET", &assets_route(identifier, query), None).await
}

/// The `Assets` one answer reports, in the order it reported them.
fn assets(output: &Value, case: &str) -> Vec<Value> {
    output
        .get("Assets")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected an Assets list, got {output}"))
}

/// The `AssetId`s one answer reports, in the order it reported them.
fn asset_ids(output: &Value, case: &str) -> Vec<String> {
    assets(output, case)
        .iter()
        .map(|asset| text(asset, "AssetId"))
        .collect()
}

/// A member of one asset's `ComputeAttributes`, or `None` for an asset that reports no
/// compute attributes at all.
fn compute<'a>(asset: &'a Value, member: &str) -> Option<&'a Value> {
    asset.get("ComputeAttributes").and_then(|it| it.get(member))
}

/// A string item of a list, read as the empty string when it is not one.
fn item_text(item: &Value) -> String {
    item.as_str().unwrap_or_default().to_string()
}

/// The `InstanceType`s one `GetOutpostSupportedInstanceTypes` answer reports.
fn supported_types(output: &Value) -> Vec<String> {
    output
        .get("InstanceTypes")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(|item| text(item, "InstanceType")).collect())
        .unwrap_or_default()
}

/// Every instance family this service offers on `form_factor` hardware, and the vCPU
/// count of the largest instance type among them: what rule 2 requires a compute unit
/// of that hardware to report, computed from the service's own specification table
/// rather than spelled out.
///
/// The table rather than the catalog `ListOrderableInstanceTypes` publishes, because
/// what hardware RUNS is every size of a family it carries while that catalog
/// publishes the one size capacity is ordered in (see
/// [`crate::instance_type_catalogs`]).
fn offered(form_factor: &str) -> (BTreeSet<String>, i64) {
    let entries = crate::instance_type_catalogs::specifications();
    assert!(
        !entries.is_empty(),
        "the service must specify instance types for its hardware to run"
    );
    let offered = entries.iter().filter(|entry| {
        entry
            .get("FormFactorConfigs")
            .and_then(Value::as_array)
            .is_some_and(|configs| {
                configs
                    .iter()
                    .any(|config| text(config, "FormFactor") == form_factor)
            })
    });
    let mut families = BTreeSet::new();
    let mut max_vcpus = 0;
    for entry in offered {
        let instance_type = text(entry, "InstanceType");
        // The family is the part before the size, as `ComputeAttributes$InstanceFamilies`
        // names it; an entry that carries no size at all names no family.
        let Some((family, _)) = instance_type.split_once('.') else {
            continue;
        };
        families.insert(family.to_string());
        max_vcpus = max_vcpus.max(entry.get("VCPUs").and_then(Value::as_i64).unwrap_or_default());
    }
    (families, max_vcpus)
}

/// Rules 1 and 2: over every form factor an Outpost can carry, what an order's
/// fulfilment installs on it, and what each installed unit reports.
#[tokio::test]
async fn an_outpost_settles_holding_the_hardware_of_its_form_factor() {
    let outposts = Outposts::new();

    // Nothing has been ordered for these Outposts, so nothing has been installed on
    // them: an Outpost holds only hardware an order delivered.
    let mut provisioning = Vec::new();
    for form_factor in HARDWARE_TYPES {
        let case = format!("{form_factor:?} while provisioning");
        let outpost_id = an_outpost(
            &outposts,
            &format!("hw-{}", form_factor.unwrap_or("none")),
            *form_factor,
        )
        .await;
        let reply = list(&outposts, &outpost_id, "").await;
        assert!(
            assets(&reply, &case).is_empty(),
            "case {case:?}: an Outpost nothing has been ordered for holds no hardware, \
             got {reply}"
        );
        provisioning.push((case, outpost_id, *form_factor));
    }

    for (_, outpost_id, _) in &provisioning {
        install_hardware(&outposts, outpost_id).await;
    }

    for (case, outpost_id, form_factor) in &provisioning {
        let case = &format!("{case} once installed");
        // The Outpost has reached its settled lifecycle status, which is the transition
        // its ordered hardware arrives with.
        let outpost = outposts.get_outpost(outpost_id).await;
        assert_eq!(
            text(&outpost, "LifeCycleStatus"),
            "ACTIVE",
            "case {case:?}: the Outpost must have settled, got {outpost}"
        );

        let reply = list(&outposts, outpost_id, "").await;
        let installed = assets(&reply, case);
        let Some(form_factor) = form_factor else {
            // An Outpost whose form factor names no configuration is installed with
            // nothing at all.
            assert!(
                installed.is_empty(),
                "case {case:?}: an Outpost that recorded no form factor names no hardware \
                 configuration, so none is installed, got {reply}"
            );
            continue;
        };
        assert!(
            !installed.is_empty(),
            "case {case:?}: a settled Outpost holds the hardware of its configuration, got \
             {reply}"
        );

        let form_factor = *form_factor;
        let racked = form_factor == "RACK";
        let (families, max_vcpus) = offered(form_factor);
        let mut types: BTreeSet<String> = BTreeSet::new();
        let mut elevations: Vec<f64> = Vec::new();
        let mut racks: BTreeSet<String> = BTreeSet::new();
        for (index, asset) in installed.iter().enumerate() {
            let case = &format!("{case} (asset {index})");
            // Rule 2's identifier half: exactly the ten digits `AssetIdInput` takes.
            let asset_id = text(asset, "AssetId");
            assert!(
                asset_id.len() == 10 && asset_id.bytes().all(|byte| byte.is_ascii_digit()),
                "case {case:?}: an AssetId must be the ten digits AssetIdInput takes, got \
                 {asset_id:?}"
            );
            let asset_type = text(asset, "AssetType");
            assert!(
                ASSET_TYPE_VALUES.contains(&asset_type.as_str()),
                "case {case:?}: an AssetType must be a value the model declares, got \
                 {asset_type:?}"
            );
            types.insert(asset_type.clone());

            match asset.get("RackId") {
                Some(rack) => {
                    let rack = rack.as_str().unwrap_or_default().to_string();
                    assert!(
                        racked && (5..=20).contains(&rack.len()),
                        "case {case:?}: only a rack configuration reports a RackId, of the \
                         modelled 5..20 length, got {rack:?}"
                    );
                    racks.insert(rack);
                }
                None => assert!(
                    !racked,
                    "case {case:?}: every unit of one rack of hardware reports its rack, got \
                     {asset}"
                ),
            }

            match asset.get("AssetLocation") {
                Some(location) => {
                    let elevation = location
                        .get("RackElevation")
                        .and_then(Value::as_f64)
                        .unwrap_or(-1.0);
                    assert!(
                        racked
                            && (RACK_ELEVATION_MIN..=RACK_ELEVATION_MAX).contains(&elevation),
                        "case {case:?}: a unit in a rack sits at a position inside \
                         RackElevation's range, got {location}"
                    );
                    elevations.push(elevation);
                }
                None => assert!(
                    !racked,
                    "case {case:?}: a unit installed in a rack reports its position, got {asset}"
                ),
            }

            // Rule 2's compute half: ComputeAttributes belong to COMPUTE hardware and
            // to nothing else.
            if asset_type != "COMPUTE" {
                assert!(
                    asset.get("ComputeAttributes").is_none(),
                    "case {case:?}: a unit that is not compute hardware reports no compute \
                     attributes, got {asset}"
                );
                continue;
            }
            let host_id = compute(asset, "HostId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert!(
                host_id.starts_with("h-")
                    && host_id.len() == 19
                    && host_id[2..].bytes().all(|byte| byte.is_ascii_hexdigit()),
                "case {case:?}: a compute unit hosts a Dedicated Host, got {host_id:?}"
            );
            assert_eq!(
                compute(asset, "State").and_then(Value::as_str),
                Some(COMPUTE_STATE_ACTIVE),
                "case {case:?}: installed compute hardware is available for new compute \
                 resources, got {asset}"
            );
            let reported: BTreeSet<String> = compute(asset, "InstanceFamilies")
                .and_then(Value::as_array)
                .map(|items| items.iter().map(item_text).collect())
                .unwrap_or_default();
            assert_eq!(
                reported, families,
                "case {case:?}: a compute unit runs the families published as orderable on its \
                 hardware, got {asset}"
            );
            assert_eq!(
                compute(asset, "MaxVcpus").and_then(Value::as_i64),
                Some(max_vcpus),
                "case {case:?}: a compute unit's vCPU ceiling is the largest orderable instance \
                 type on its hardware ({max_vcpus}), got {asset}"
            );
            // "The instance type capacities configured for this asset. This can be
            // changed through a capacity task": nothing has configured this host.
            assert!(
                compute(asset, "InstanceTypeCapacities").is_none(),
                "case {case:?}: a host nothing has configured reports no per-type capacity, got \
                 {asset}"
            );
        }

        if racked {
            // A rack is a WHOLE rack of hardware: every kind of unit the modelled
            // `AssetType` enum declares is installed in it, each at its own position,
            // bottom-up, and all of them in the one rack.
            for asset_type in ASSET_TYPE_VALUES {
                assert!(
                    types.contains(*asset_type),
                    "case {case:?}: a rack of hardware holds a unit of every modelled asset \
                     type, got {types:?}"
                );
            }
            assert_eq!(
                racks.len(),
                1,
                "case {case:?}: one rack of hardware is one rack, got {racks:?}"
            );
            assert!(
                elevations.windows(2).all(|pair| pair[0] < pair[1]),
                "case {case:?}: no two units share a position and they are reported bottom-up, \
                 got {elevations:?}"
            );
        } else {
            // "An Outpost asset can be a single server within an Outposts rack or an
            // Outposts server configuration": a server Outpost is the one server.
            assert_eq!(
                installed.len(),
                1,
                "case {case:?}: a server Outpost holds the one server it is, got {reply}"
            );
            assert_eq!(
                types,
                BTreeSet::from(["COMPUTE".to_string()]),
                "case {case:?}: the server an Outposts server is, is compute hardware, got \
                 {types:?}"
            );
        }
    }
}

/// Rule 3: hardware is installed once and belongs to one Outpost — repeated reads
/// answer the same assets, and two Outposts of one store never share an asset id.
#[tokio::test]
async fn hardware_is_installed_once_and_belongs_to_one_outpost() {
    let outposts = Outposts::new();
    let mut created = Vec::new();
    // Two Outposts of EACH form factor, so the rows differ along both the form factor
    // and whether another Outpost of the same configuration exists.
    for form_factor in HARDWARE_TYPES {
        for ordinal in ["first", "second"] {
            let name = format!("once-{}-{ordinal}", form_factor.unwrap_or("none"));
            created.push((name.clone(), an_outpost(&outposts, &name, *form_factor).await));
        }
    }

    for (_, outpost_id) in &created {
        install_hardware(&outposts, outpost_id).await;
    }

    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (case, outpost_id) in &created {
        let first = list(&outposts, outpost_id, "").await;
        // Reading an Outpost's hardware installs nothing further: the clock installed
        // it once, as the Outpost settled.
        for pass in ["second read", "third read"] {
            let again = list(&outposts, outpost_id, "").await;
            assert_eq!(
                again, first,
                "case {case:?} ({pass}): every read of an Outpost's hardware answers the same"
            );
        }
        let later = list(&outposts, outpost_id, "").await;
        assert_eq!(
            later, first,
            "case {case:?}: hardware is installed once, so a later read answers the same"
        );
        for asset_id in asset_ids(&first, case) {
            assert!(
                seen.insert(asset_id.clone()),
                "case {case:?}: an asset belongs to one Outpost, so {asset_id:?} must be held \
                 by one Outpost only"
            );
        }
    }
    assert!(
        !seen.is_empty(),
        "the rows must have installed hardware to compare"
    );
}

/// Rule 4: an asset id read out of `ListAssets` is one a caller can then name —
/// `GetOutpostSupportedInstanceTypes` scoped to it, and `StartCapacityTask` for it.
#[tokio::test]
async fn an_installed_asset_id_is_usable_where_the_api_takes_one() {
    let outposts = Outposts::new();
    let mut created = Vec::new();
    for form_factor in ["RACK", "SERVER"] {
        created.push((
            form_factor,
            an_outpost(
                &outposts,
                &format!("named-{form_factor}"),
                Some(form_factor),
            )
            .await,
        ));
    }

    for (_, outpost_id) in &created {
        install_hardware(&outposts, outpost_id).await;
    }

    for (form_factor, outpost_id) in &created {
        let reply = list(&outposts, outpost_id, "").await;
        let installed = assets(&reply, form_factor);
        // The whole report of the Outpost itself: what one of its units can support is
        // what that unit's own compute attributes admit of it.
        let whole = supported_types(
            &outposts
                .ok("GET", &supported_route(outpost_id, "?MaxResults=1000"), None)
                .await,
        );
        assert!(
            !whole.is_empty(),
            "case {form_factor:?}: the Outpost itself must support instance types, got {whole:?}"
        );

        for asset in &installed {
            let asset_id = text(asset, "AssetId");
            let asset_type = text(asset, "AssetType");
            let case = &format!("{form_factor} {asset_type} {asset_id}");
            let scoped = supported_types(
                &outposts
                    .ok(
                        "GET",
                        &supported_route(
                            outpost_id,
                            &format!("?MaxResults=1000&AssetId={}", query_value(&asset_id)),
                        ),
                        None,
                    )
                    .await,
            );
            if asset_type == "COMPUTE" {
                assert_eq!(
                    scoped, whole,
                    "case {case:?}: a compute unit runs what its Outpost's hardware runs"
                );
            } else {
                assert!(
                    scoped.is_empty(),
                    "case {case:?}: a unit that hosts no instance supports no instance type, \
                     got {scoped:?}"
                );
            }
        }

        // The same id names the hardware a capacity task is for. One task at a time
        // runs on an Outpost, so one compute unit of each is named — with a pool of an
        // instance type the Outpost supports, so nothing but the asset id is in
        // question.
        let host = installed
            .iter()
            .find(|asset| text(asset, "AssetType") == "COMPUTE")
            .map(|asset| text(asset, "AssetId"))
            .unwrap_or_else(|| {
                panic!("case {form_factor:?}: the configuration must hold a compute unit")
            });
        let started = outposts
            .ok(
                "POST",
                &capacity_route(outpost_id),
                Some(&json!({
                    "InstancePools": [{ "InstanceType": whole[0], "Count": 1 }],
                    "AssetId": host,
                })),
            )
            .await;
        assert_eq!(
            text(&started, "AssetId"),
            host,
            "case {form_factor:?}: the task must be for the asset the request named, got {started}"
        );
    }
}

/// Rule 5: with hardware installed, the answer's filters and its page bound are both
/// observable over the wire.
///
/// The filter rows differ along which member each filter selects on and how many
/// values it names; the paging rows along the page size, which spans one unit per
/// page, a size that leaves a short last page, and more than the whole list.
#[tokio::test]
async fn filters_and_paging_are_observable_on_installed_hardware() {
    let outposts = Outposts::new();
    let outpost_id = an_outpost(&outposts, "installed-filters", Some("RACK")).await;
    install_hardware(&outposts, &outpost_id).await;

    let whole = list(&outposts, &outpost_id, "").await;
    let installed = assets(&whole, "installed");
    let every_id = asset_ids(&whole, "installed");
    assert!(
        installed.len() > 2,
        "the rack configuration must hold enough units to page through, got {whole}"
    );
    let ids_of = |types: &[&str]| -> Vec<String> {
        installed
            .iter()
            .filter(|asset| types.contains(&text(asset, "AssetType").as_str()))
            .map(|asset| text(asset, "AssetId"))
            .collect()
    };
    let compute_ids = ids_of(&["COMPUTE"]);
    let storage_ids = ids_of(&["STORAGE"]);
    let both_ids = ids_of(&["COMPUTE", "STORAGE"]);
    let one_host = installed
        .iter()
        .find_map(|asset| compute(asset, "HostId").and_then(Value::as_str))
        .expect("a rack of hardware holds a compute unit with a host")
        .to_string();
    let one_host_ids: Vec<String> = installed
        .iter()
        .filter(|asset| compute(asset, "HostId").and_then(Value::as_str) == Some(one_host.as_str()))
        .map(|asset| text(asset, "AssetId"))
        .collect();

    let cases: Vec<(String, String, Vec<String>)> = vec![
        (
            "the compute units".to_string(),
            "?AssetTypeFilter=COMPUTE".to_string(),
            compute_ids.clone(),
        ),
        (
            "the storage units".to_string(),
            "?AssetTypeFilter=STORAGE".to_string(),
            storage_ids,
        ),
        (
            "two asset types, so the union of both".to_string(),
            "?AssetTypeFilter=COMPUTE&AssetTypeFilter=STORAGE".to_string(),
            both_ids,
        ),
        (
            "the state installed compute hardware is in".to_string(),
            format!("?StatusFilter={COMPUTE_STATE_ACTIVE}"),
            compute_ids.clone(),
        ),
        (
            "a state no installed unit is in".to_string(),
            "?StatusFilter=RETIRING".to_string(),
            Vec::new(),
        ),
        (
            "one host of the installed hardware".to_string(),
            format!("?HostIdFilter={}", query_value(&one_host)),
            one_host_ids,
        ),
        (
            "a host id no unit carries".to_string(),
            "?HostIdFilter=h-00000000000000000".to_string(),
            Vec::new(),
        ),
        (
            "an asset type and a state that narrow rather than widen".to_string(),
            format!("?AssetTypeFilter=SWITCH&StatusFilter={COMPUTE_STATE_ACTIVE}"),
            Vec::new(),
        ),
    ];

    for (case, query, wanted) in &cases {
        let reply = list(&outposts, &outpost_id, query).await;
        assert_eq!(
            &asset_ids(&reply, case),
            wanted,
            "case {case:?} ({query}): ListAssets must answer only the matching units, got {reply}"
        );
    }

    for size in [1usize, 2, 5, installed.len(), installed.len() + 3] {
        let case = &format!("page size {size}");
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let query = match &token {
                Some(token) => format!("?MaxResults={size}&NextToken={}", query_value(token)),
                None => format!("?MaxResults={size}"),
            };
            let page = list(&outposts, &outpost_id, &query).await;
            let page_ids = asset_ids(&page, case);
            assert!(
                page_ids.len() <= size,
                "case {case:?}: a page holds at most MaxResults assets, got {page}"
            );
            collected.extend(page_ids);
            pages += 1;
            assert!(
                pages <= installed.len() + 1,
                "case {case:?}: paging must terminate, got {collected:?}"
            );
            match page.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            &collected, &every_id,
            "case {case:?}: paging must yield every installed unit once, in installation order"
        );
        if size < installed.len() {
            assert!(
                pages > 1,
                "case {case:?}: a page bound below the whole list must be observable, got \
                 {pages} page(s)"
            );
        }
    }
}
