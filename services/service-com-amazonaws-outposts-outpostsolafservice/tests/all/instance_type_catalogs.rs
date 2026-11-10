// SPDX-License-Identifier: CC0-1.0

//! WHICH instance types this service publishes as orderable, driven over the wire
//! through the same front door as every other suite (the harness in
//! [`crate::site_and_outpost_lifecycle`]).
//!
//! Two different lists meet here, and this suite is what keeps them apart. The one
//! `ListOrderableInstanceTypes` PUBLISHES is what EC2 capacity can be ORDERED in; the
//! sizes this service holds SPECIFICATIONS for is the wider list every other report
//! of an instance type is answered from. [`crate::orderable_instance_types`] owns what
//! one published entry reports and how the list filters and pages; this suite owns its
//! membership.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. The catalog publishes one entry per RACK instance family this service offers,
//!    naming that family's WHOLE HOST — the largest size it is offered in, which is
//!    the size one physical host of the family is, and the size the published capacity
//!    of a hardware configuration counts in (`EC2Capacity$MaxSize` with its
//!    `Quantity`).
//! 2. A size SMALLER than a family's whole host is not published as orderable — it is
//!    a slice of a host already installed rather than capacity to order — and this
//!    service still holds its specification: an Outpost whose hardware runs the family
//!    supports it, and a capacity task is weighed in it.
//! 3. A family offered only on SERVER hardware is not published as orderable either: a
//!    server is one host, ordered as a whole unit from the hardware catalog, so its
//!    capacity is published on the SERVER configurations of that catalog instead.
//! 4. Every published entry is offered in the RACK form factor, and both modelled
//!    Outpost generations are published.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::state::instance_type_specifications;

use crate::capacity_task_lifecycle::{install_capacity, outpost, pool, start_ok, INSTALLED};
use crate::site_and_outpost_lifecycle::{text, Outposts};

/// Every instance type this service holds the hardware specifications of, shaped
/// exactly as a read of a catalog reports an entry.
///
/// The specification table is reference data no operation publishes whole — what
/// `ListOrderableInstanceTypes` publishes is the narrower list of whole hosts — so the
/// suites whose expectations are about a SIZE read it from the service's own table
/// here rather than keeping a copy of it.
pub(crate) fn specifications() -> Vec<Value> {
    instance_type_specifications()
        .into_iter()
        .map(|record| {
            let entry = record.instance_type;
            json!({
                "InstanceType": entry.instance_type,
                "VCPUs": entry.vcp_us,
                "MemoryInMib": entry.memory_in_mib,
                "NetworkPerformance": entry.network_performance,
                "FormFactorConfigs": entry
                    .form_factor_configs
                    .unwrap_or_default()
                    .into_iter()
                    .map(|config| {
                        json!({
                            "FormFactor": config.form_factor,
                            "OutpostGeneration": config.outpost_generation,
                        })
                    })
                    .collect::<Vec<Value>>(),
            })
        })
        .collect()
}

/// The whole host of every RACK family this service offers: `(instance type, VCPUs,
/// MemoryInMib, OutpostGeneration)`.
///
/// Source: the EC2 instance-type specifications AWS publishes for each of these
/// sizes — the largest size of a first-generation rack family is `24xlarge` (96 vCPU:
/// `c5`/`c5d` 192 GiB, `m5`/`m5d` 384 GiB, `r5`/`r5d` and `i3en` 768 GiB), of a
/// seventh-generation Intel family `48xlarge` (192 vCPU: `c7i` 384 GiB, `m7i` 768 GiB,
/// `r7i` 1536 GiB) and of an eighth-generation one `96xlarge` (384 vCPU: `c8i` 768
/// GiB, `m8i` 1536 GiB, `r8i` 3072 GiB) — together with the Outpost generation of the
/// hardware each family is offered on. The rows span both modelled generations and all
/// four of the compute-, general-purpose-, memory- and storage-optimized families, so
/// an entry the catalog gained or lost shows up here rather than in one spot check.
const WHOLE_HOSTS: &[(&str, i64, i64, &str)] = &[
    ("c5.24xlarge", 96, 196_608, "GENERATION_1"),
    ("c5d.24xlarge", 96, 196_608, "GENERATION_1"),
    ("c7i.48xlarge", 192, 393_216, "GENERATION_2"),
    ("c8i.96xlarge", 384, 786_432, "GENERATION_2"),
    ("i3en.24xlarge", 96, 786_432, "GENERATION_1"),
    ("m5.24xlarge", 96, 393_216, "GENERATION_1"),
    ("m5d.24xlarge", 96, 393_216, "GENERATION_1"),
    ("m7i.48xlarge", 192, 786_432, "GENERATION_2"),
    ("m8i.96xlarge", 384, 1_572_864, "GENERATION_2"),
    ("r5.24xlarge", 96, 786_432, "GENERATION_1"),
    ("r5d.24xlarge", 96, 786_432, "GENERATION_1"),
    ("r7i.48xlarge", 192, 1_572_864, "GENERATION_2"),
    ("r8i.96xlarge", 384, 3_145_728, "GENERATION_2"),
];

/// Sizes SMALLER than their family's whole host, one per kind of family and spanning
/// the smallest size, an odd-numbered one and the size just below a whole host.
///
/// Source: the same published EC2 specifications — each is a real size of a family the
/// catalog offers, and none of them is that family's largest.
const SLICES: &[&str] = &[
    "c5.large",
    "c5.9xlarge",
    "c5d.4xlarge",
    "i3en.3xlarge",
    "m5.4xlarge",
    "m5d.2xlarge",
    "r5.16xlarge",
    "r5d.12xlarge",
    "c7i.24xlarge",
    "m7i.12xlarge",
    "r7i.16xlarge",
    "c8i.48xlarge",
    "m8i.32xlarge",
    "r8i.8xlarge",
];

/// The families this service offers on SERVER hardware alone, each with the whole host
/// a server of it is: an Outposts server is built on the Graviton2 `c6gd` (16xlarge,
/// 64 vCPU) or the Intel `c6id` (32xlarge, 128 vCPU), as the published EC2
/// specifications size those families.
const SERVER_HOSTS: &[(&str, &str)] = &[("c6gd", "16xlarge"), ("c6id", "32xlarge")];

/// The modelled `OutpostGeneration` values (`GENERATION_1` | `GENERATION_2`), as
/// `smithy_query(query="shape", shape="OutpostGeneration")` reports them.
const GENERATIONS: &[&str] = &["GENERATION_1", "GENERATION_2"];

/// The `InstanceTypes` list of one answer.
fn entries(output: &Value) -> Vec<Value> {
    output
        .get("InstanceTypes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The whole published catalog, as `ListOrderableInstanceTypes` reports it.
async fn published(outposts: &Outposts, query: &str) -> Vec<Value> {
    entries(&outposts.ok("GET", &format!("/instanceTypes{query}"), None).await)
}

/// The instance family of an instance type: `c5` in `c5.24xlarge`.
fn family(instance_type: &str) -> &str {
    instance_type.split('.').next().unwrap_or_default()
}

/// The `(FormFactor, OutpostGeneration)` pairs one entry's `FormFactorConfigs` names.
fn form_factor_configs(entry: &Value) -> Vec<(String, String)> {
    entry
        .get("FormFactorConfigs")
        .and_then(Value::as_array)
        .map(|configs| {
            configs
                .iter()
                .map(|config| {
                    (
                        text(config, "FormFactor"),
                        text(config, "OutpostGeneration"),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The largest vCPU count this service holds a specification of for `family`.
fn largest_specified_vcpus(table: &[Value], family_name: &str) -> i64 {
    table
        .iter()
        .filter(|entry| family(&text(entry, "InstanceType")) == family_name)
        .filter_map(|entry| entry.get("VCPUs").and_then(Value::as_i64))
        .max()
        .unwrap_or_default()
}

/// Rule 1: the catalog is exactly one whole host per RACK family, and the size each
/// entry names really is the largest this service holds a specification of for that
/// family.
#[tokio::test]
async fn the_catalog_publishes_one_whole_host_per_rack_family() {
    let outposts = Outposts::new();
    let catalog = published(&outposts, "?MaxResults=1000").await;
    let table = specifications();

    let reported: Vec<(String, i64, i64)> = catalog
        .iter()
        .map(|entry| {
            (
                text(entry, "InstanceType"),
                entry.get("VCPUs").and_then(Value::as_i64).unwrap_or(-1),
                entry
                    .get("MemoryInMib")
                    .and_then(Value::as_i64)
                    .unwrap_or(-1),
            )
        })
        .collect();
    let expected: Vec<(String, i64, i64)> = WHOLE_HOSTS
        .iter()
        .map(|(instance_type, vcpus, memory_in_mib, _)| {
            ((*instance_type).to_string(), *vcpus, *memory_in_mib)
        })
        .collect();
    assert_eq!(
        reported, expected,
        "the catalog must publish the whole host of every rack family, with its \
         published hardware, in one stable order"
    );

    // One entry per family, and the size it names is that family's largest — asked of
    // the service's own specifications, so the table above cannot be what decides it.
    let mut per_family: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for entry in &catalog {
        let instance_type = text(entry, "InstanceType");
        let case = instance_type.clone();
        let family_name = family(&instance_type).to_string();
        let vcpus = entry.get("VCPUs").and_then(Value::as_i64).unwrap_or(-1);
        assert_eq!(
            vcpus,
            largest_specified_vcpus(&table, &family_name),
            "case {case:?}: a published entry is its family's whole host, so its VCPUs \
             must be the largest this service specifies for {family_name:?}, got {entry:?}"
        );
        per_family.entry(family_name).or_default().push(instance_type);
    }
    for (family_name, published_sizes) in &per_family {
        assert_eq!(
            published_sizes.len(),
            1,
            "case {family_name:?}: a family is published in its whole host alone, got \
             {published_sizes:?}"
        );
    }
}

/// Rule 2: a slice of a host is not orderable, yet the service still sizes it — the
/// Outpost that runs the family supports it, and a capacity task is weighed in it.
#[tokio::test]
async fn a_size_below_the_whole_host_is_not_orderable_but_is_still_specified() {
    let outposts = Outposts::new();
    let catalog = published(&outposts, "?MaxResults=1000").await;
    let orderable: BTreeSet<String> = catalog
        .iter()
        .map(|entry| text(entry, "InstanceType"))
        .collect();
    let table = specifications();

    // Hardware that runs any family up to its own vCPU ceiling, so what the report
    // below reports is decided by what this service specifies and not by a family the
    // asset recorded.
    let outpost_id = outpost(&outposts, "whole-host-slices").await;
    install_capacity(&outposts, &outpost_id, &INSTALLED, "whole-host-slices");
    let supported: BTreeSet<String> = entries(
        &outposts
            .ok(
                "GET",
                &format!("/outposts/{outpost_id}/supportedInstanceTypes?MaxResults=1000"),
                None,
            )
            .await,
    )
    .iter()
    .map(|entry| text(entry, "InstanceType"))
    .collect();

    for case in SLICES {
        let specified = table
            .iter()
            .find(|entry| text(entry, "InstanceType") == *case)
            .unwrap_or_else(|| {
                panic!("case {case:?}: must be a size this service holds a specification of")
            });
        let vcpus = specified.get("VCPUs").and_then(Value::as_i64).unwrap_or(-1);
        assert!(
            vcpus > 0,
            "case {case:?}: a specified size carries the vCPU count of that size, got \
             {specified:?}"
        );
        assert!(
            !orderable.contains(*case),
            "case {case:?}: a size below its family's whole host is not published as \
             orderable, got {orderable:?}"
        );
        assert!(
            orderable
                .iter()
                .any(|published| family(published) == family(case)),
            "case {case:?}: its family is published in its whole host, got {orderable:?}"
        );
        // A size that fits the installed hosts is one the Outpost supports, which is
        // what makes the specification reachable rather than merely held.
        if vcpus <= i64::from(crate::capacity_task_lifecycle::HOST_VCPUS) {
            assert!(
                supported.contains(*case),
                "case {case:?}: an Outpost whose hardware runs it supports it, got \
                 {supported:?}"
            );
            // And one the service can weigh a configuration in: a dry run of a single
            // host of it is answered, which it cannot be for a size no vCPU count is
            // published for.
            let weighed = start_ok(
                &outposts,
                &outpost_id,
                &json!({ "InstancePools": [pool(case, 1)], "DryRun": true }),
            )
            .await;
            assert_eq!(
                text(&weighed, "CapacityTaskStatus"),
                "COMPLETED",
                "case {case:?}: a single host of a size the hardware runs is weighed as \
                 holdable, got {weighed:?}"
            );
        }
    }
}

/// Rule 3: a SERVER-only family is not published as orderable, and the hardware
/// catalog publishes its capacity instead.
#[tokio::test]
async fn a_server_only_family_is_published_as_hardware_rather_than_as_orderable() {
    let outposts = Outposts::new();
    let orderable: BTreeSet<String> = published(&outposts, "?MaxResults=1000")
        .await
        .iter()
        .map(|entry| family(&text(entry, "InstanceType")).to_string())
        .collect();
    let servers = outposts
        .ok(
            "GET",
            "/catalog/items?ItemClassFilter=SERVER&MaxResults=1000",
            None,
        )
        .await;
    let items = servers
        .get("CatalogItems")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        !items.is_empty(),
        "the hardware catalog must publish SERVER configurations, got {servers:?}"
    );

    let mut published_capacity: BTreeSet<(String, String)> = BTreeSet::new();
    for item in &items {
        let case = text(item, "CatalogItemId");
        for capacity in item
            .get("EC2Capacities")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let pair = (text(&capacity, "Family"), text(&capacity, "MaxSize"));
            assert!(
                SERVER_HOSTS.contains(&(pair.0.as_str(), pair.1.as_str())),
                "case {case:?}: a SERVER configuration carries the capacity of the \
                 hardware a server is built on, got {capacity:?}"
            );
            published_capacity.insert(pair);
        }
    }

    for (family_name, max_size) in SERVER_HOSTS {
        assert!(
            !orderable.contains(*family_name),
            "case {family_name:?}: a family offered on SERVER hardware alone is not \
             published as orderable capacity, got {orderable:?}"
        );
        assert!(
            published_capacity.contains(&((*family_name).to_string(), (*max_size).to_string())),
            "case {family_name:?}: a SERVER configuration publishes its capacity, got \
             {published_capacity:?}"
        );
    }
}

/// Rule 4: where a published entry can be ordered — the RACK form factor, in the
/// generation of its own hardware, over both modelled generations.
#[tokio::test]
async fn every_published_entry_is_rack_capacity_of_its_generation() {
    let outposts = Outposts::new();
    let catalog = published(&outposts, "?MaxResults=1000").await;
    let generations: BTreeMap<&str, &str> = WHOLE_HOSTS
        .iter()
        .map(|(instance_type, _, _, generation)| (*instance_type, *generation))
        .collect();

    for entry in &catalog {
        let case = text(entry, "InstanceType");
        let expected = generations.get(case.as_str()).unwrap_or_else(|| {
            panic!("case {case:?}: must be one of the published whole hosts")
        });
        assert_eq!(
            form_factor_configs(entry),
            vec![("RACK".to_string(), (*expected).to_string())],
            "case {case:?}: capacity is ordered on rack hardware of the generation its \
             family is built on, got {entry:?}"
        );
    }

    for generation in GENERATIONS {
        let filtered = published(
            &outposts,
            &format!("?MaxResults=1000&OutpostGenerationFilter={generation}"),
        )
        .await;
        let reported: Vec<String> = filtered
            .iter()
            .map(|entry| text(entry, "InstanceType"))
            .collect();
        let expected: Vec<String> = WHOLE_HOSTS
            .iter()
            .filter(|(_, _, _, offered)| offered == generation)
            .map(|(instance_type, ..)| (*instance_type).to_string())
            .collect();
        assert_eq!(
            reported, expected,
            "case {generation:?}: every whole host of that generation is published for it"
        );
    }
}
