// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetOutpostSupportedInstanceTypes`, driven over the wire
//! through the same front door as every other suite (the harness in
//! [`crate::site_and_outpost_lifecycle`]).
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. What an Outpost can support is every instance type this service holds a
//!    specification of (the wider of its two instance-type lists, which every
//!    expectation below is computed from rather than spelled out — the catalog
//!    `ListOrderableInstanceTypes` publishes is the narrower one entry per family
//!    capacity is ordered in, see [`crate::instance_type_catalogs`]) narrowed by every
//!    hardware fact the store records about the scope the request names, and nothing
//!    else:
//!    * the hardware INSTALLED on the Outpost is what the Outpost itself can support,
//!      so an entry is supported when one of its installed compute units can host it;
//!      an Outpost holding no compute hardware supports nothing and reports no entry
//!      at all;
//!    * the Outpost's `SupportedHardwareType` is the form factor its hardware has, so
//!      an entry is supported only where its `FormFactorConfigs` offers it in that
//!      form factor; an Outpost that recorded no hardware type narrows nothing;
//!    * an `AssetId` scopes the report to one asset of that Outpost: an asset that
//!      recorded no `ComputeAttributes` is not compute hardware and supports none, and
//!      one that did is narrowed to the instance families it records and to the vCPU
//!      count its `MaxVcpus` admits — a fact it did not record narrowing nothing.
//!
//!    Each reported entry carries the `InstanceType` and the `VCPUs` this service
//!    specifies for it, so the capability report cannot disagree with any other report
//!    of that instance type.
//! 2. The report is cut into pages of at most `MaxResults` with a `NextToken` that
//!    resumes exactly where the previous page stopped, for any page size, scoped or
//!    not; the last page carries no token, and a token this operation did not issue —
//!    including one the catalog list issued — is a bad request.
//! 3. It is a REPORT: both identifier forms the modelled `OutpostIdentifier` pattern
//!    accepts answer the same thing, repeating a call answers the same thing, and the
//!    recorded hardware is left exactly as it was.
//! 4. An identifier that names nothing this store holds is 404 `NotFoundException`
//!    with the identifier echo of the value AS SUPPLIED — the Outpost, an `AssetId`
//!    that names none of its assets, and an `OrderId` this store holds no order of
//!    this Outpost for (what an order of it scopes the report to is exercised in
//!    [`crate::supported_instance_types_order_scope`]). One that is no identifier at
//!    all is 400 `ValidationException`.
//!
//! Where the recorded assets come from: an asset is a PHYSICAL unit of hardware that
//! Amazon Web Services installs, so no call in this API creates one — the rows below
//! therefore record their own hardware on the stored Outpost, which nothing has been
//! ordered for, through the same store seam [`crate::outpost_assets`] records its
//! assets through, and then ask over the wire. What a unit of the hardware an Outpost
//! settles with reports, and what this report answers when scoped to one of those
//! units, is [`crate::outpost_hardware_installation`].

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::ops::types::{
    AssetInfo, ComputeAttributes,
};
use service_com_amazonaws_outposts_outpostsolafservice::state::{
    nested_page_kind, SUPPORTED_INSTANCE_TYPE_PAGE,
};

use crate::site_and_outpost_lifecycle::{
    orderable_site, query_value, refusal, segment, text, Outposts, Refusal, ABSENT_OUTPOST_ID,
    OUTPOST_ARN_PREFIX,
};

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

/// The `VCPUs` one reported entry carries, or `-1` when it carries none — which no
/// catalog entry does, so a missing count fails a row rather than being skipped.
fn vcpus(item: &Value) -> i64 {
    item.get("VCPUs").and_then(Value::as_i64).unwrap_or(-1)
}

/// Every instance type this service holds a specification of: the source every
/// expectation below is computed from.
///
/// Not the catalog `ListOrderableInstanceTypes` publishes, which is the narrower list
/// of the one size per family capacity is ORDERED in: what an Outpost supports is
/// every size its hardware runs, so the wider table is what this report is narrowed
/// from (see [`crate::instance_type_catalogs`]).
fn catalog() -> Vec<Value> {
    crate::instance_type_catalogs::specifications()
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
    let instance_type = text(entry, "InstanceType");
    instance_type
        .split('.')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The hardware facts one COMPUTE unit in scope records: what the rule narrows the
/// catalog by for that unit.
#[derive(Debug, Clone, Copy, Default)]
struct ComputeScope<'a> {
    /// The instance families the unit records, or `None` when it recorded none.
    families: Option<&'a [&'a str]>,
    /// The unit's `MaxVcpus`, or `None` when none is recorded.
    max_vcpus: Option<i64>,
}

/// The hardware facts a row records about the scope it asks about: what the rule
/// narrows the catalog by.
#[derive(Debug, Clone)]
struct Scope<'a> {
    /// The Outpost's `SupportedHardwareType`, or `None` for an Outpost that recorded
    /// none.
    form_factor: Option<&'a str>,
    /// The COMPUTE hardware in scope: the one asset a request named, or every compute
    /// unit installed on the Outpost when it named none. EMPTY for a scope that holds
    /// no compute hardware at all — an Outpost nothing is installed on, or an asset
    /// that is not compute hardware — which can host no instance.
    hardware: Vec<ComputeScope<'a>>,
}

impl<'a> Scope<'a> {
    /// The Outpost itself: its form factor, and the compute units installed on it.
    fn outpost(form_factor: Option<&'a str>, hardware: &[ComputeScope<'a>]) -> Self {
        Self {
            form_factor,
            hardware: hardware.to_vec(),
        }
    }

    /// ONE asset of it, which is compute hardware recording `families` and
    /// `max_vcpus`.
    fn asset(
        form_factor: Option<&'a str>,
        families: Option<&'a [&'a str]>,
        max_vcpus: Option<i64>,
    ) -> Self {
        Self {
            form_factor,
            hardware: vec![ComputeScope {
                families,
                max_vcpus,
            }],
        }
    }

    /// A scope that is not compute hardware at all, which supports nothing.
    fn not_compute(form_factor: Option<&'a str>) -> Self {
        Self {
            form_factor,
            hardware: Vec::new(),
        }
    }
}

/// What rule 1 requires for `scope`, computed from the published catalog.
fn expected(catalog: &[Value], scope: &Scope<'_>) -> Vec<(String, i64)> {
    catalog
        .iter()
        .filter(|entry| {
            scope
                .form_factor
                .is_none_or(|form_factor| offered_in(entry, form_factor))
                && scope.hardware.iter().any(|unit| {
                    unit.families
                        .is_none_or(|families| families.contains(&family(entry).as_str()))
                        && unit.max_vcpus.is_none_or(|max| vcpus(entry) <= max)
                })
        })
        .map(|entry| (text(entry, "InstanceType"), vcpus(entry)))
        .collect()
}

/// One site and one Outpost on it carrying `hardware_type`, as that Outpost's
/// `(OutpostId, OutpostArn)`.
async fn an_outpost(
    outposts: &Outposts,
    name: &str,
    hardware_type: Option<&str>,
) -> (String, String) {
    let site = outposts
        .create_site(&orderable_site(&format!("{name}-site")))
        .await;
    let mut request = json!({ "Name": name, "SiteId": text(&site, "SiteId") });
    if let Some(hardware_type) = hardware_type {
        request["SupportedHardwareType"] = json!(hardware_type);
    }
    let outpost = outposts.create_outpost(&request).await;
    (text(&outpost, "OutpostId"), text(&outpost, "OutpostArn"))
}

/// One recorded asset: its `AssetId`, its `AssetType`, and the compute hardware it
/// recorded (`None` for an asset that is not compute hardware at all).
fn asset(id: &str, asset_type: &str, compute: Option<ComputeAttributes>) -> AssetInfo {
    AssetInfo {
        asset_id: Some(id.to_string()),
        rack_id: Some("rack-01".to_string()),
        asset_type: Some(asset_type.to_string()),
        compute_attributes: compute,
        asset_location: None,
    }
}

/// The `ComputeAttributes` of one recorded compute asset: the instance families and
/// the maximum vCPU count it recorded, each absent where the row records none.
fn compute(families: Option<&[&str]>, max_vcpus: Option<i32>) -> ComputeAttributes {
    ComputeAttributes {
        host_id: Some("h-0123456789abcdef".to_string()),
        state: Some("ACTIVE".to_string()),
        instance_families: families
            .map(|families| families.iter().map(|family| family.to_string()).collect()),
        instance_type_capacities: None,
        max_vcpus,
    }
}

/// Record `assets` as the hardware installed on the stored Outpost. See this module's
/// header for why a row records them through the store.
fn record(outposts: &Outposts, outpost_id: &str, assets: &[AssetInfo], case: &str) {
    let store = outposts.store();
    let mut store = store.lock();
    let record = store
        .outposts
        .get_mut(outpost_id)
        .unwrap_or_else(|| panic!("case {case:?}: the create persisted the Outpost"));
    record.assets = assets.to_vec();
}

/// Walk the report with `MaxResults=size`, following every `NextToken` to the end.
///
/// `extra` is whatever else the query carries (an asset scope, or nothing).
async fn walk(
    outposts: &Outposts,
    identifier: &str,
    size: usize,
    extra: &str,
    ceiling: usize,
) -> Vec<(String, i64)> {
    let mut collected: Vec<(String, i64)> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let query = match &token {
            Some(token) => format!("?MaxResults={size}{extra}&NextToken={}", query_value(token)),
            None => format!("?MaxResults={size}{extra}"),
        };
        let page = supported(outposts, identifier, &query).await;
        let entries = reported(&page);
        assert!(
            entries.len() <= size,
            "case size {size}{extra}: a page holds at most MaxResults entries, got {} entries",
            entries.len()
        );
        collected.extend(entries);
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

/// Rule 1's Outpost half: the report is the catalog narrowed to the hardware
/// INSTALLED on the Outpost and to its form factor — over every hardware type an
/// Outpost in this store can carry, and over installations that differ along what the
/// rule generalises over: none at all, one compute unit, two of them (the report is
/// their union), and hardware that hosts no instance.
#[tokio::test]
async fn an_outpost_supports_the_hardware_installed_on_it() {
    let outposts = Outposts::new();
    let catalog = catalog();
    assert!(
        !catalog.is_empty(),
        "the published catalog must hold entries to narrow, got {catalog:?}"
    );

    // Each case is one installation: the units recorded on the Outpost, and the
    // compute facts the expectation narrows the catalog by. The `AssetId` of each
    // unit is a value the modelled `AssetIdInput` (`^\d{10}$`) accepts.
    let installations: Vec<(&str, Vec<AssetInfo>, Vec<ComputeScope<'_>>)> = vec![
        ("nothing installed", Vec::new(), Vec::new()),
        (
            "one compute unit, no facts recorded",
            vec![asset("3000000001", "COMPUTE", Some(compute(None, None)))],
            vec![ComputeScope::default()],
        ),
        (
            "one compute unit, one family and a ceiling",
            vec![asset(
                "3000000002",
                "COMPUTE",
                Some(compute(Some(&["c5"]), Some(48))),
            )],
            vec![ComputeScope {
                families: Some(&["c5"]),
                max_vcpus: Some(48),
            }],
        ),
        (
            "two compute units: the report is their union",
            vec![
                asset(
                    "3000000003",
                    "COMPUTE",
                    Some(compute(Some(&["c5"]), Some(16))),
                ),
                asset(
                    "3000000004",
                    "COMPUTE",
                    Some(compute(Some(&["m5", "r5"]), None)),
                ),
            ],
            vec![
                ComputeScope {
                    families: Some(&["c5"]),
                    max_vcpus: Some(16),
                },
                ComputeScope {
                    families: Some(&["m5", "r5"]),
                    max_vcpus: None,
                },
            ],
        ),
        (
            "only hardware that hosts no instance",
            vec![
                asset("3000000005", "STORAGE", None),
                asset("3000000006", "POWERSHELF", None),
            ],
            Vec::new(),
        ),
    ];

    for hardware_type in HARDWARE_TYPES {
        for (installation, units, scope) in &installations {
            let case = format!("{installation} on {hardware_type:?}");
            let (outpost_id, outpost_arn) = an_outpost(
                &outposts,
                &format!(
                    "hw-{}-{}",
                    hardware_type.unwrap_or("none"),
                    installation.len()
                ),
                *hardware_type,
            )
            .await;
            record(&outposts, &outpost_id, units, &case);
            let want = expected(&catalog, &Scope::outpost(*hardware_type, scope));
            let answer = supported(&outposts, &outpost_id, "").await;
            assert_eq!(
                reported(&answer),
                want,
                "case {case:?}: expected the catalog narrowed to the installed hardware, got \
                 {answer:?}"
            );
            assert!(
                answer.get("NextToken").is_none(),
                "case {case:?}: a page that exhausts the report issues no NextToken, got \
                 {answer:?}"
            );
            // Rule 3's identifier half: the ARN form names the same Outpost.
            assert_eq!(
                reported(&supported(&outposts, &outpost_arn, "").await),
                want,
                "case {case:?}: the ARN form must answer the same report as the bare id"
            );
        }
    }

    // Hardware is what makes an entry supported: an Outpost holding none supports
    // nothing, and one holding a compute unit that records no narrowing fact supports
    // everything its form factor offers.
    for hardware_type in HARDWARE_TYPES {
        assert!(
            expected(&catalog, &Scope::not_compute(*hardware_type)).is_empty(),
            "case {hardware_type:?}: hardware that hosts no instance supports no instance type"
        );
        let whole = expected(
            &catalog,
            &Scope::outpost(*hardware_type, &[ComputeScope::default()]),
        );
        assert!(
            !whole.is_empty(),
            "case {hardware_type:?}: every form factor the catalog names must offer something"
        );
    }

    // The form factors narrow rather than widen: each one is a strict subset of what
    // an Outpost with no recorded hardware type reports off the same hardware.
    let unnarrowed = expected(&catalog, &Scope::outpost(None, &[ComputeScope::default()]));
    for form_factor in ["RACK", "SERVER"] {
        let narrowed = expected(
            &catalog,
            &Scope::outpost(Some(form_factor), &[ComputeScope::default()]),
        );
        assert!(
            narrowed.len() < unnarrowed.len()
                && narrowed.iter().all(|entry| unnarrowed.contains(entry)),
            "case {form_factor:?}: a form factor must narrow the catalog, got {} of {} entries",
            narrowed.len(),
            unnarrowed.len()
        );
    }
}

/// One recorded asset a row asks about: the hardware it records, and the facts the
/// expectation narrows the catalog by.
struct Row {
    /// What this row is a case of, named in every assertion it makes.
    case: &'static str,
    /// The asset's `AssetId`, which is also what the request scopes the report by.
    asset_id: &'static str,
    /// The asset's `AssetType`.
    asset_type: &'static str,
    /// The `ComputeAttributes` the asset records, or `None` for an asset that is not
    /// compute hardware.
    hardware: Option<ComputeAttributes>,
    /// The instance families that hardware records, if any.
    families: Option<&'static [&'static str]>,
    /// The `MaxVcpus` that hardware records, if any.
    max_vcpus: Option<i64>,
}

/// Rule 1's asset half: an `AssetId` scopes the report to that asset's own recorded
/// hardware, over rows that differ along every fact the rule narrows by — which
/// families (one, two, none), which vCPU ceiling (none, a ceiling that cuts the
/// catalog, one below its smallest entry), an asset that is not compute hardware at
/// all, and a family the Outpost's form factor does not offer — and on an Outpost of
/// each form factor, so the two clauses are exercised together.
#[tokio::test]
async fn an_asset_scope_narrows_to_the_hardware_that_asset_recorded() {
    let outposts = Outposts::new();
    let catalog = catalog();

    // Each row is one asset: the hardware it records, and the facts the expectation
    // narrows the catalog by. The `AssetId` of each row is a value the modelled
    // `AssetIdInput` (`^\d{10}$`) accepts.
    let rows: Vec<Row> = vec![
        Row {
            case: "one family, no ceiling",
            asset_id: "1000000001",
            asset_type: "COMPUTE",
            hardware: Some(compute(Some(&["c5"]), None)),
            families: Some(&["c5"]),
            max_vcpus: None,
        },
        Row {
            case: "two families and a ceiling that cuts them",
            asset_id: "1000000002",
            asset_type: "COMPUTE",
            hardware: Some(compute(Some(&["c5", "m5"]), Some(16))),
            families: Some(&["c5", "m5"]),
            max_vcpus: Some(16),
        },
        Row {
            case: "no family recorded, a ceiling only",
            asset_id: "1000000003",
            asset_type: "COMPUTE",
            hardware: Some(compute(None, Some(8))),
            families: None,
            max_vcpus: Some(8),
        },
        Row {
            case: "compute hardware that recorded neither",
            asset_id: "1000000004",
            asset_type: "COMPUTE",
            hardware: Some(compute(None, None)),
            families: None,
            max_vcpus: None,
        },
        Row {
            case: "a family only one form factor offers",
            asset_id: "1000000005",
            asset_type: "COMPUTE",
            hardware: Some(compute(Some(&["c6gd"]), None)),
            families: Some(&["c6gd"]),
            max_vcpus: None,
        },
        Row {
            case: "a ceiling below the smallest entry",
            asset_id: "1000000006",
            asset_type: "COMPUTE",
            hardware: Some(compute(None, Some(0))),
            families: None,
            max_vcpus: Some(0),
        },
        Row {
            case: "an empty family list narrows nothing",
            asset_id: "1000000007",
            asset_type: "COMPUTE",
            hardware: Some(compute(Some(&[]), None)),
            families: None,
            max_vcpus: None,
        },
        Row {
            case: "storage hardware, no compute attributes",
            asset_id: "1000000008",
            asset_type: "STORAGE",
            hardware: None,
            families: None,
            max_vcpus: None,
        },
    ];

    let installed: Vec<AssetInfo> = rows
        .iter()
        .map(|row| asset(row.asset_id, row.asset_type, row.hardware.clone()))
        .collect();

    for form_factor in HARDWARE_TYPES.iter().copied() {
        let (outpost_id, _) = an_outpost(
            &outposts,
            &format!("assets-{}", form_factor.unwrap_or("none")),
            form_factor,
        )
        .await;
        record(&outposts, &outpost_id, &installed, "asset scopes");

        for row in &rows {
            let case = format!("{} on {form_factor:?}", row.case);
            let want = match row.hardware.is_none() {
                true => expected(&catalog, &Scope::not_compute(form_factor)),
                false => expected(
                    &catalog,
                    &Scope::asset(form_factor, row.families, row.max_vcpus),
                ),
            };
            let answer = supported(
                &outposts,
                &outpost_id,
                &format!("?AssetId={}", query_value(row.asset_id)),
            )
            .await;
            assert_eq!(
                reported(&answer),
                want,
                "case {case:?}: expected the catalog narrowed to that asset's hardware, got \
                 {answer:?}"
            );
        }

        // Scoping to an asset never widens the report: every asset's answer is part of
        // what the Outpost itself reports off the same installation.
        let units: Vec<ComputeScope<'_>> = rows
            .iter()
            .filter(|row| row.hardware.is_some())
            .map(|row| ComputeScope {
                families: row.families,
                max_vcpus: row.max_vcpus,
            })
            .collect();
        let whole = expected(&catalog, &Scope::outpost(form_factor, &units));
        for row in &rows {
            let answer = reported(
                &supported(
                    &outposts,
                    &outpost_id,
                    &format!("?AssetId={}", query_value(row.asset_id)),
                )
                .await,
            );
            assert!(
                answer.iter().all(|entry| whole.contains(entry)),
                "case {:?} on {form_factor:?}: an asset scope must narrow, got {answer:?}",
                row.case
            );
        }
    }

    // The rows are not all the same answer: a family scope, a ceiling scope and
    // non-compute hardware each report something different on a RACK Outpost.
    let (outpost_id, _) = an_outpost(&outposts, "assets-distinct", Some("RACK")).await;
    record(&outposts, &outpost_id, &installed, "distinct");
    let one_family = reported(&supported(&outposts, &outpost_id, "?AssetId=1000000001").await);
    let ceiling = reported(&supported(&outposts, &outpost_id, "?AssetId=1000000003").await);
    let storage = reported(&supported(&outposts, &outpost_id, "?AssetId=1000000008").await);
    assert!(
        !one_family.is_empty() && !ceiling.is_empty() && one_family != ceiling,
        "a family scope and a ceiling scope must select different hardware, got \
         {one_family:?} and {ceiling:?}"
    );
    assert!(
        storage.is_empty(),
        "hardware that hosts no instance supports no instance type, got {storage:?}"
    );
}

/// Rule 2 + rule 3: for EVERY page size, scoped and unscoped, paging from the first
/// `NextToken` to the last yields exactly what one unpaged call reports, once each, in
/// the same order — and the unpaged call answers the same thing after the walk as
/// before it, with the recorded hardware untouched.
#[tokio::test]
async fn paging_covers_every_supported_instance_type_exactly_once() {
    let outposts = Outposts::new();
    let (outpost_id, _) = an_outpost(&outposts, "paging", Some("RACK")).await;
    // Two compute units, so the Outpost-wide report is strictly larger than the
    // report scoped to one of them and both are worth paging.
    let installed = vec![
        asset(
            "2000000001",
            "COMPUTE",
            Some(compute(Some(&["c5", "m5", "r5"]), Some(48))),
        ),
        asset("2000000002", "COMPUTE", Some(compute(None, None))),
    ];
    record(&outposts, &outpost_id, &installed, "paging");

    let before = supported(&outposts, &outpost_id, "").await;
    let whole = reported(&before);
    let count = whole.len();
    let scoped = reported(&supported(&outposts, &outpost_id, "?AssetId=2000000001").await);
    assert!(
        !scoped.is_empty() && scoped.len() < count,
        "the scoped report must be a smaller non-empty report to page, got {} of {count}",
        scoped.len()
    );

    // One per page, a small size, a size that leaves a short last page, the count
    // itself and more than the count — so a token that skipped or duplicated an entry
    // at any boundary would show up.
    for size in [1usize, 7, 13, count, count + 5] {
        assert_eq!(
            walk(&outposts, &outpost_id, size, "", count).await,
            whole,
            "case size {size}: paging must yield the whole report once, in order"
        );
        assert_eq!(
            walk(&outposts, &outpost_id, size, "&AssetId=2000000001", count).await,
            scoped,
            "case size {size}: paging a scoped report must yield it once, in order"
        );
    }

    // Rule 3: nothing the reads did changed the answer or the recorded hardware.
    assert_eq!(
        supported(&outposts, &outpost_id, "").await,
        before,
        "reading the report must not change what a later caller sees"
    );
    let held = outposts.store().lock().outposts[&outpost_id].assets.clone();
    assert_eq!(
        held.len(),
        installed.len(),
        "reporting supported instance types must leave the recorded hardware alone, got {held:?}"
    );

    // The token this report issues resumes after the page that issued it.
    let first = supported(&outposts, &outpost_id, "?MaxResults=1").await;
    let token = text(&first, "NextToken");
    assert!(
        !token.is_empty(),
        "a report of {count} entries served one at a time issues a NextToken, got {first:?}"
    );
    let second = supported(
        &outposts,
        &outpost_id,
        &format!("?MaxResults=1&NextToken={}", query_value(&token)),
    )
    .await;
    assert_ne!(
        reported(&second),
        reported(&first),
        "the token must resume AFTER the page that issued it, got {second:?}"
    );

    // A boundary past the end of the catalog is an empty last page, not a refusal.
    // The boundary a token names is a position in the published catalog, so the
    // past-the-end boundary is past the whole catalog rather than past this narrowed
    // report; the report is this Outpost's own, so the stamp the token carries names
    // the Outpost as well as the kind of report (`state::nested_page_kind`).
    let catalog_entries = catalog().len();
    let past_the_end = supported(
        &outposts,
        &outpost_id,
        &format!(
            "?NextToken={}",
            query_value(&format!(
                "{}##{}",
                catalog_entries + 1,
                nested_page_kind(SUPPORTED_INSTANCE_TYPE_PAGE, &[&outpost_id])
            ))
        ),
    )
    .await;
    assert_eq!(
        (reported(&past_the_end).len(), past_the_end.get("NextToken")),
        (0, None),
        "a boundary past the end of the report is an empty last page, got {past_the_end:?}"
    );

    // A token this report did not issue, and every page size outside the modelled
    // `MaxResults1000` range, are bad requests.
    let cases = vec![
        refusal(
            "a token stamped for ListOrderableInstanceTypes",
            "GET",
            route(&outpost_id, "?NextToken=1%23%23instanceTypes"),
            400,
            "ValidationException",
        ),
        refusal(
            "a token stamped for ListAssets",
            "GET",
            route(&outpost_id, "?NextToken=1%23%23assets"),
            400,
            "ValidationException",
        ),
        refusal(
            "a well-formed token with a nonsense boundary",
            "GET",
            route(&outpost_id, "?NextToken=abc%23%23supportedInstanceTypes"),
            400,
            "ValidationException",
        ),
        refusal(
            "a page size below the modelled minimum",
            "GET",
            route(&outpost_id, "?MaxResults=0"),
            400,
            "ValidationException",
        ),
        refusal(
            "a page size above the modelled maximum",
            "GET",
            route(&outpost_id, "?MaxResults=1001"),
            400,
            "ValidationException",
        ),
    ];
    outposts.expect_refusals(&cases).await;

    // The other direction: this report's token is not a boundary of the catalog list.
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
        "a supported-instance-types token replayed on the catalog list is refused, got {:?}",
        replayed.body
    );
}

/// Rule 4: what the store does not hold, and what is no identifier at all.
#[tokio::test]
async fn a_scope_this_store_does_not_hold_is_refused() {
    let outposts = Outposts::new();
    let (outpost_id, _) = an_outpost(&outposts, "refusals", Some("RACK")).await;
    record(
        &outposts,
        &outpost_id,
        &[asset(
            "3000000001",
            "COMPUTE",
            Some(compute(Some(&["c5"]), None)),
        )],
        "refusals",
    );
    let absent_outpost_arn = format!("{OUTPOST_ARN_PREFIX}{ABSENT_OUTPOST_ID}");
    // An order id the modelled `OrderId` pattern (`^oo-[a-f0-9]{17}$`) accepts.
    let order_id = "oo-0123456789abcdef0";

    // Each refusal carries the wording the KIND of resource it is about: the echo of
    // the Outpost identifier as supplied on this read route, the asset sentence for an
    // asset of it, and the order sentence for an order.
    let bodies: Vec<(&str, String, String)> = vec![
        (
            "an Outpost nothing holds, by bare id",
            route(ABSENT_OUTPOST_ID, ""),
            format!("outpostIdentifier={ABSENT_OUTPOST_ID}"),
        ),
        (
            "an Outpost nothing holds, by ARN",
            route(&absent_outpost_arn, ""),
            format!("outpostIdentifier={absent_outpost_arn}"),
        ),
        (
            "an AssetId none of its assets carries",
            route(&outpost_id, "?AssetId=9999999999"),
            "No such asset id 9999999999 found.".to_string(),
        ),
        (
            "an order this store holds no record of",
            route(&outpost_id, &format!("?OrderId={order_id}")),
            format!("No such order for order id {order_id}"),
        ),
        (
            "an order named beside an asset that does exist",
            route(
                &outpost_id,
                &format!("?OrderId={order_id}&AssetId=3000000001"),
            ),
            format!("No such order for order id {order_id}"),
        ),
    ];
    for (case, path, expected) in &bodies {
        let reply = outposts.call("GET", path, None).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (404, "NotFoundException"),
            "case {case:?}: expected 404 NotFoundException, got {} {} {:?}",
            reply.status,
            reply.error_code,
            reply.body
        );
        assert_eq!(
            text(&reply.body, "Message"),
            *expected,
            "case {case:?}: expected $.Message {expected:?}, got {:?}",
            reply.body
        );
    }

    // An Outpost that existed and was then deleted is gone to this report too.
    let (deleted, _) = an_outpost(&outposts, "deleted", Some("RACK")).await;
    outposts
        .ok("DELETE", &format!("/outposts/{}", segment(&deleted)), None)
        .await;

    // Anything that is no identifier at all, or no value the modelled scope members
    // accept, is a bad request rather than an empty report.
    let cases: Vec<Refusal> = vec![
        refusal(
            "an Outpost identifier that is not one",
            "GET",
            route("op-not-an-outpost", ""),
            400,
            "ValidationException",
        ),
        refusal(
            "a site id where an Outpost identifier belongs",
            "GET",
            route("os-00000000000000000", ""),
            400,
            "ValidationException",
        ),
        refusal(
            "an AssetId that is not ten digits",
            "GET",
            route(&outpost_id, "?AssetId=abc"),
            400,
            "ValidationException",
        ),
        refusal(
            "an AssetId of the wrong length",
            "GET",
            route(&outpost_id, "?AssetId=123456789"),
            400,
            "ValidationException",
        ),
        refusal(
            "an OrderId the modelled pattern refuses",
            "GET",
            route(&outpost_id, "?OrderId=oo-not-an-order"),
            400,
            "ValidationException",
        ),
        refusal(
            "an Outpost that was deleted",
            "GET",
            route(&deleted, ""),
            404,
            "NotFoundException",
        ),
    ];
    outposts.expect_refusals(&cases).await;
}
