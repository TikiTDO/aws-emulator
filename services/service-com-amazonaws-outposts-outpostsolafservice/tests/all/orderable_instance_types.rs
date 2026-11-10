// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListOrderableInstanceTypes`, driven over the wire through the
//! same front door as every other suite (the harness in
//! [`crate::site_and_outpost_lifecycle`]).
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. The operation reports the orderable-instance-type CATALOG the service
//!    publishes: every entry names an EC2 instance type `<family>.<size>` whose
//!    `VCPUs` is the count that size names and whose `MemoryInMib` is that count
//!    times the family's published memory-per-vCPU ratio, with a non-empty
//!    `NetworkPerformance` and a non-empty `FormFactorConfigs` list whose every pair
//!    is a modelled `(FormFactor, OutpostGeneration)`. WHICH entries that catalog
//!    holds — the whole host of each rack family, one per family — is
//!    [`crate::instance_type_catalogs`].
//! 2. `OutpostGenerationFilter` selects exactly the entries one of whose
//!    `FormFactorConfigs` names that generation, and reports each selected entry
//!    WHOLE; an absent filter selects the whole catalog.
//! 3. The catalog is cut into pages of at most `MaxResults` with a `NextToken` that
//!    resumes exactly where the previous page stopped, for any page size and with or
//!    without a filter; the last page carries no token.
//! 4. Reading the catalog changes nothing: the same call answers the same thing every
//!    time, before and after a whole paged walk.
//! 5. A `NextToken` this service did not issue for THIS list is a bad request (400
//!    `ValidationException`), as is a `MaxResults` outside the modelled range; a
//!    boundary past the end of the catalog is an empty last page, not a refusal.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::site_and_outpost_lifecycle::{query_value, text, Outposts};

/// The modelled `OutpostGeneration` values (`GENERATION_1` | `GENERATION_2`), as
/// `smithy_query(query="shape", shape="OutpostGeneration")` reports them.
const GENERATIONS: &[&str] = &["GENERATION_1", "GENERATION_2"];

/// The modelled `FormFactor` values (`RACK` | `SERVER`), from the same model.
const FORM_FACTORS: &[&str] = &["RACK", "SERVER"];

/// How many MiB one GiB is, which is the unit `MemoryInMib` reports in.
const MIB_PER_GIB: i64 = 1024;

/// The memory, in GiB per vCPU, that every size of an instance FAMILY carries.
///
/// Source: the EC2 instance-type specifications AWS publishes — a compute-optimized
/// family (`c5`, `c5d`, `c6gd`, `c6id`, `c7i`, `c8i`) carries 2 GiB per vCPU, a
/// general-purpose one (`m5`, `m5d`, `m7i`, `m8i`) 4, and a memory- or
/// storage-optimized one (`r5`, `r5d`, `r7i`, `r8i`, `i3en`) 8; a `d`-suffix family is
/// its base family with local NVMe storage, so it carries the same figures. The rows
/// span every family the service offers, so an entry whose family is missing here
/// fails rule 1 rather than being skipped.
const FAMILY_MEMORY_GIB_PER_VCPU: &[(&str, i64)] = &[
    ("c5", 2),
    ("c5d", 2),
    ("c6gd", 2),
    ("c6id", 2),
    ("c7i", 2),
    ("c8i", 2),
    ("i3en", 8),
    ("m5", 4),
    ("m5d", 4),
    ("m7i", 4),
    ("m8i", 4),
    ("r5", 8),
    ("r5d", 8),
    ("r7i", 8),
    ("r8i", 8),
];

/// Entries rule 1 is spot-checked against, none of them the one the issue cited:
/// `(instance type, vCPUs, MemoryInMib)`.
///
/// Source: the published EC2 specifications for each of these instance types —
/// `c5.24xlarge` 96 vCPU / 192 GiB, `c5d.24xlarge` 96 / 192, `i3en.24xlarge` 96 / 768,
/// `m5.24xlarge` 96 / 384, `m5d.24xlarge` 96 / 384, `r5.24xlarge` 96 / 768,
/// `r5d.24xlarge` 96 / 768, `c7i.48xlarge` 192 / 384, `m7i.48xlarge` 192 / 768,
/// `r7i.48xlarge` 192 / 1536 and `r8i.96xlarge` 384 / 3072 — which span both Outpost
/// generations and the compute-, general-purpose-, memory- and storage-optimized
/// families.
const SPOT_CHECKS: &[(&str, i64, i64)] = &[
    ("c5.24xlarge", 96, 192 * MIB_PER_GIB),
    ("c5d.24xlarge", 96, 192 * MIB_PER_GIB),
    ("i3en.24xlarge", 96, 768 * MIB_PER_GIB),
    ("m5.24xlarge", 96, 384 * MIB_PER_GIB),
    ("m5d.24xlarge", 96, 384 * MIB_PER_GIB),
    ("r5.24xlarge", 96, 768 * MIB_PER_GIB),
    ("r5d.24xlarge", 96, 768 * MIB_PER_GIB),
    ("c7i.48xlarge", 192, 384 * MIB_PER_GIB),
    ("m7i.48xlarge", 192, 768 * MIB_PER_GIB),
    ("r7i.48xlarge", 192, 1536 * MIB_PER_GIB),
    ("r8i.96xlarge", 384, 3072 * MIB_PER_GIB),
];

/// The vCPU count an EC2 instance SIZE names.
///
/// Source: the EC2 instance-type naming rule the published specifications follow —
/// `medium` is 1 vCPU, `large` 2, `xlarge` 4, and `<n>xlarge` is n times the
/// `xlarge` count. `None` for a suffix that is not one of those forms.
fn vcpus_for_size(size: &str) -> Option<i64> {
    match size {
        "medium" => Some(1),
        "large" => Some(2),
        "xlarge" => Some(4),
        _ => size
            .strip_suffix("xlarge")
            .and_then(|multiple| multiple.parse::<i64>().ok())
            .map(|multiple| multiple * 4),
    }
}

/// The `InstanceTypes` list of one answer.
fn entries(output: &Value) -> Vec<Value> {
    output
        .get("InstanceTypes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `InstanceType` of every entry of one answer, in the order it reported them.
fn names(output: &Value) -> Vec<String> {
    entries(output)
        .iter()
        .map(|entry| text(entry, "InstanceType"))
        .collect()
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

/// `ListOrderableInstanceTypes` with the given query string, requiring HTTP 200.
async fn list(outposts: &Outposts, query: &str) -> Value {
    outposts
        .ok("GET", &format!("/instanceTypes{query}"), None)
        .await
}

/// Walk the catalog with `MaxResults=size`, following every `NextToken` to the end,
/// and answer the instance types the walk collected.
///
/// `extra` is whatever else the query carries (a generation filter, or nothing).
async fn walk(outposts: &Outposts, size: usize, extra: &str, ceiling: usize) -> Vec<String> {
    let mut collected: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let query = match &token {
            Some(token) => format!(
                "?MaxResults={size}{extra}&NextToken={}",
                query_value(token)
            ),
            None => format!("?MaxResults={size}{extra}"),
        };
        let page = list(outposts, &query).await;
        let page_names = names(&page);
        assert!(
            page_names.len() <= size,
            "case size {size}{extra}: a page holds at most MaxResults entries, got {} entries",
            page_names.len()
        );
        collected.extend(page_names);
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

/// Rule 1: every entry of the catalog reports hardware consistent with its family's
/// published ratio and its size's published vCPU count, plus the modelled form-factor
/// configurations — checked over the WHOLE catalog, not one row, and spot-checked
/// against [`SPOT_CHECKS`].
#[tokio::test]
async fn every_catalog_entry_reports_consistent_modelled_hardware() {
    let outposts = Outposts::new();
    let all = list(&outposts, "").await;
    let catalog = entries(&all);
    let ratios: BTreeMap<&str, i64> = FAMILY_MEMORY_GIB_PER_VCPU.iter().copied().collect();

    assert!(
        catalog.len() >= SPOT_CHECKS.len(),
        "the catalog must hold at least the spot-checked entries, got {} entries",
        catalog.len()
    );
    assert!(
        all.get("NextToken").is_none(),
        "a page that exhausts the catalog issues no NextToken, got {all:?}"
    );

    for entry in &catalog {
        let name = text(entry, "InstanceType");
        let case = name.clone();
        let (family, size) = name
            .split_once('.')
            .unwrap_or_else(|| panic!("case {case:?}: InstanceType must be <family>.<size>"));
        let ratio = ratios.get(family).copied().unwrap_or_else(|| {
            panic!("case {case:?}: {family:?} is not a family with a published ratio")
        });
        let expected_vcpus = vcpus_for_size(size).unwrap_or_else(|| {
            panic!("case {case:?}: {size:?} is not a published EC2 instance size")
        });
        let vcpus = entry.get("VCPUs").and_then(Value::as_i64);
        assert_eq!(
            vcpus,
            Some(expected_vcpus),
            "case {case:?}: VCPUs must be what the size names, got {entry:?}"
        );
        assert_eq!(
            entry.get("MemoryInMib").and_then(Value::as_i64),
            Some(expected_vcpus * ratio * MIB_PER_GIB),
            "case {case:?}: MemoryInMib must be VCPUs times {ratio} GiB, got {entry:?}"
        );
        assert!(
            !text(entry, "NetworkPerformance").is_empty(),
            "case {case:?}: NetworkPerformance must be reported, got {entry:?}"
        );
        let configs = form_factor_configs(entry);
        assert!(
            !configs.is_empty(),
            "case {case:?}: an orderable instance type names where it can be ordered, \
             got {entry:?}"
        );
        for (form_factor, generation) in &configs {
            assert!(
                FORM_FACTORS.contains(&form_factor.as_str())
                    && GENERATIONS.contains(&generation.as_str()),
                "case {case:?}: ({form_factor:?}, {generation:?}) must be a modelled \
                 FormFactorConfig, got {entry:?}"
            );
        }
        assert_eq!(
            configs.iter().collect::<BTreeSet<_>>().len(),
            configs.len(),
            "case {case:?}: no form-factor configuration is reported twice, got {entry:?}"
        );
    }

    // The catalog is a set, reported in one stable order.
    let reported = names(&all);
    let mut sorted = reported.clone();
    sorted.sort();
    assert_eq!(
        reported, sorted,
        "the catalog is reported in one stable (name) order"
    );
    assert_eq!(
        reported.iter().collect::<BTreeSet<_>>().len(),
        reported.len(),
        "no instance type is reported twice, got {reported:?}"
    );

    for (instance_type, vcpus, memory_in_mib) in SPOT_CHECKS {
        let entry = catalog
            .iter()
            .find(|entry| text(entry, "InstanceType") == *instance_type)
            .unwrap_or_else(|| panic!("case {instance_type:?}: must be in the catalog"));
        assert_eq!(
            (
                entry.get("VCPUs").and_then(Value::as_i64),
                entry.get("MemoryInMib").and_then(Value::as_i64),
            ),
            (Some(*vcpus), Some(*memory_in_mib)),
            "case {instance_type:?}: must report its published hardware, got {entry:?}"
        );
    }
}

/// Rule 2: the generation filter selects on the entry's own `FormFactorConfigs`, over
/// every modelled generation, and leaves the selected entries untouched.
#[tokio::test]
async fn the_generation_filter_selects_entries_orderable_in_that_generation() {
    let outposts = Outposts::new();
    let all = list(&outposts, "").await;
    let catalog = entries(&all);

    for generation in GENERATIONS {
        let expected: Vec<Value> = catalog
            .iter()
            .filter(|entry| {
                form_factor_configs(entry)
                    .iter()
                    .any(|(_, offered)| offered == generation)
            })
            .cloned()
            .collect();
        let filtered = list(&outposts, &format!("?OutpostGenerationFilter={generation}")).await;
        assert!(
            !expected.is_empty(),
            "case {generation:?}: the catalog must offer something for every modelled generation"
        );
        assert_eq!(
            entries(&filtered),
            expected,
            "case {generation:?}: the filter must select exactly the entries orderable in it, \
             and report each one whole"
        );
    }

    // Every entry is selected by every generation its own configurations name, so an
    // entry offered in both is reported under both.
    for entry in &catalog {
        let name = text(entry, "InstanceType");
        for (_, generation) in form_factor_configs(entry) {
            let filtered = list(&outposts, &format!("?OutpostGenerationFilter={generation}")).await;
            assert!(
                names(&filtered).contains(&name),
                "case {name:?}: must be reported for {generation:?}, its own configuration"
            );
        }
    }

    // A value outside the modelled enum is a bad request, not an empty page — an
    // unknown generation and the empty string alike.
    for value in ["GENERATION_9", ""] {
        let refused = outposts
            .call(
                "GET",
                &format!("/instanceTypes?OutpostGenerationFilter={value}"),
                None,
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_code.as_str()),
            (400, "ValidationException"),
            "case {value:?}: a generation outside the modelled enum is refused, got {} {:?}",
            refused.status,
            refused.body
        );
    }
}

/// Rule 3 + rule 4: for EVERY page size, and with and without a filter, paging from
/// the first `NextToken` to the last yields exactly the entries that one unpaged call
/// reports, once each, in the same order — and the unpaged call answers the same
/// thing after the whole walk as before it.
#[tokio::test]
async fn paging_the_catalog_covers_every_entry_exactly_once() {
    let outposts = Outposts::new();
    let before = list(&outposts, "").await;
    let whole = names(&before);
    let count = whole.len();

    // One per page, a small size, a size that leaves a short last page, the count
    // itself and more than the count — so a token that skipped or duplicated an entry
    // at any boundary would show up.
    let sizes = [1usize, 7, 13, count, count + 5];
    for size in sizes {
        assert_eq!(
            walk(&outposts, size, "", count).await,
            whole,
            "case size {size}: paging must yield the whole catalog once, in order"
        );
        for generation in GENERATIONS {
            let filter = format!("&OutpostGenerationFilter={generation}");
            let expected = names(&list(&outposts, &format!("?OutpostGenerationFilter={generation}")).await);
            assert_eq!(
                walk(&outposts, size, &filter, count).await,
                expected,
                "case size {size} {generation:?}: paging a filtered catalog must yield it once"
            );
        }
    }

    // Rule 4: nothing the reads did changed the catalog.
    assert_eq!(
        list(&outposts, "").await,
        before,
        "reading the catalog must not change what a later caller sees"
    );
}

/// Rule 5: what a pagination token and a page size may be.
#[tokio::test]
async fn a_token_this_list_did_not_issue_is_refused() {
    let outposts = Outposts::new();
    let count = names(&list(&outposts, "").await).len();

    // The token this list DOES issue is honoured, and resumes after the first page.
    let first = list(&outposts, "?MaxResults=1").await;
    let token = first
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !token.is_empty(),
        "a catalog of {count} entries served one at a time issues a NextToken, got {first:?}"
    );
    let second = list(
        &outposts,
        &format!("?MaxResults=1&NextToken={}", query_value(&token)),
    )
    .await;
    assert_ne!(
        names(&second),
        names(&first),
        "the token must resume AFTER the page that issued it, got {second:?}"
    );

    // A boundary past the end of the catalog is an empty last page.
    let past_the_end = list(
        &outposts,
        &format!("?NextToken={}", query_value(&format!("{}##instanceTypes", count + 1))),
    )
    .await;
    assert_eq!(
        (names(&past_the_end).len(), past_the_end.get("NextToken")),
        (0, None),
        "a boundary past the end of the catalog is an empty last page, got {past_the_end:?}"
    );

    // Everything that is not a token for THIS list, and every page size outside the
    // modelled `MaxResults1000` range.
    let cases: Vec<(&str, String)> = vec![
        (
            "a token stamped for ListOutposts",
            "/instanceTypes?NextToken=1%23%23outposts".to_string(),
        ),
        (
            "a token stamped for ListSites",
            "/instanceTypes?NextToken=1%23%23sites".to_string(),
        ),
        (
            "a well-formed token with a nonsense boundary",
            "/instanceTypes?NextToken=abc%23%23instanceTypes".to_string(),
        ),
        (
            "a page size below the modelled minimum",
            "/instanceTypes?MaxResults=0".to_string(),
        ),
        (
            "a page size above the modelled maximum",
            "/instanceTypes?MaxResults=1001".to_string(),
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
            &format!("/outposts?NextToken={}", query_value(&token)),
            None,
        )
        .await;
    assert_eq!(
        (replayed.status, replayed.error_code.as_str()),
        (400, "ValidationException"),
        "a ListOrderableInstanceTypes token replayed on ListOutposts is refused, got {:?}",
        replayed.body
    );
}
