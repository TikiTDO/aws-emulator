// SPDX-License-Identifier: CC0-1.0

//! EVERY entry an `ListInstances` enumeration serves carries its `State` view, over the
//! `awsJson1_1` wire.
//!
//! THE RULE: the instance projection this service serves carries, beside each instance's
//! `Status`, a `State` structure whose `Name` is that same instance's lifecycle value — on
//! every entry of every page, read from the entry's own record, so no entry of an
//! enumeration can be served without the view and no entry can be served another
//! instance's.
//!
//! `instance_state_view.rs` pins that the member is served and mirrors the status through
//! the lifecycle, `listed_instance_state_source.rs` the RECORD it is read from and
//! `listed_instance_state_under_any_name.rs` the `Name` value space. This suite spans what
//! is left: the CARDINALITY of the enumeration and each entry's POSITION in it — how many
//! instances the account holds, how those instances are split into pages, and where in a
//! page an entry lands — since a selector that picks ONE entry out of a listing says
//! nothing about the others. The counts are the published bounds of the output list
//! (`smithy_query(query="shape",shape="com.amazonaws.ssoadmin#InstanceList")`: `list of
//! InstanceMetadata`, `length min 0 max 10`) plus the sizes in between and one past the
//! maximum, and the statuses the rows seed are the model-generated
//! `ops::types::INSTANCE_STATUS_VALUES`, cycled so neighbouring entries differ and each
//! position sees each value across the rows.
//!
//! The instances are seeded through the SAME `state::Instance` constructor the handlers
//! use, since an account enables one instance through this API and these rows need many;
//! everything then asserted goes over `MultiServiceRouter::dispatch`. The harness is the
//! one `ssoadmin_permission_sets.rs` owns and the rule assertion the one
//! `instance_state_view.rs` owns.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::ops::types::INSTANCE_STATUS_VALUES;
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::instance_state_view::assert_state_mirrors_status;
use crate::ssoadmin_permission_sets::{call_in_region, router_over, with_store};

/// The Region every instance below is enabled in, and the Region its calls are signed for:
/// a `RegionName` (`^([a-z]+-){2,3}\d$`) none of the sibling suites uses.
const REGION: &str = "me-south-1";

/// The published maximum of the output list (`InstanceList`, `length min 0 max 10`), which
/// is the largest listing a single page can carry.
const MAX_LISTED_INSTANCES: usize = 10;

/// The `Name` the instance at `index` carries: the member is optional and no part of an
/// instance's identity, so the rows leave it out, repeat ONE spelling across several
/// instances (so no entry can be picked out by name alone) and give the rest distinct,
/// ad-hoc generated spellings the service did not choose.
fn seeded_name(index: usize) -> Option<String> {
    match index % 3 {
        0 => None,
        1 => Some("SharedName".to_string()),
        _ => Some(format!("gen-idc-{:08x}", 0x51a9_0000_u32 + index as u32)),
    }
}

/// A router over an account holding `count` instances, with the status each one was seeded
/// in, keyed by `InstanceArn` — the map the enumeration's entries are checked against.
///
/// The statuses cycle through every value the model's `InstanceStatus` enum declares, and
/// nothing is left owed on any record (no activation and no removal instant), so the status
/// a row seeded is the status every read of that record reports.
fn seeded_account(count: usize) -> (MultiServiceRouter, BTreeMap<String, String>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let now = SystemTime::now();
    let expected = with_store(&engine, |store| {
        store.instances.clear();
        // The account's ambient instance has been enabled once and is gone, so no later
        // request enables one beside the records seeded here.
        store.ambient_instance_enabled = true;
        let mut expected = BTreeMap::new();
        for index in 0..count {
            let status = INSTANCE_STATUS_VALUES[index % INSTANCE_STATUS_VALUES.len()];
            let mut instance = state::Instance::active(
                // `ssoins-` + 16 of `[a-zA-Z0-9-.]`, as the model's `InstanceArn`
                // pattern requires.
                &format!("ssoins-{:016x}", 0x1a2b_0000_u64 + index as u64),
                // `d-` + 10 hex digits, per `InstanceIdentityStoreArn`.
                &format!("d-{:010x}", 0x009f_0000_u64 + index as u64),
                if index % 2 == 0 {
                    state::InstanceType::Account
                } else {
                    state::InstanceType::Organization
                },
                seeded_name(index),
                aws_smithy_types::DateTime::from(now),
                REGION,
            );
            instance.status = status.to_string();
            instance.active_due = None;
            instance.deleted_due = None;
            expected.insert(instance.instance_arn.clone(), status.to_string());
            store.instances.insert(instance.instance_arn.clone(), instance);
        }
        expected
    });
    (router_over(engine), expected)
}

/// Every entry `ListInstances` serves, following each `NextToken` it hands out, so an entry
/// is asserted on whichever page it arrives on.
///
/// `held` is how many instances the account holds, which bounds the number of pages: the
/// smallest page the model admits is one instance, and the enumeration ends on the page that
/// returns nothing.
async fn enumerate(
    case: &str,
    router: &MultiServiceRouter,
    max_results: Option<i64>,
    held: usize,
) -> Vec<Value> {
    let mut entries = Vec::new();
    let mut next_token: Option<String> = None;
    for _ in 0..held + 3 {
        let mut request = json!({});
        if let Some(max) = max_results {
            request["MaxResults"] = json!(max);
        }
        if let Some(token) = next_token.as_deref() {
            request["NextToken"] = json!(token);
        }
        let (status, body) = call_in_region(router, REGION, "ListInstances", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListInstances expected 200, got {status} {body}"
        );
        let page = body["Instances"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no Instances in {body}"))
            .clone();
        if let Some(max) = max_results {
            assert!(
                page.len() as i64 <= max,
                "case {case:?}: a page serves at most the MaxResults asked for, got {} \
                 entries",
                page.len()
            );
        }
        entries.extend(page.iter().cloned());
        next_token = match body["NextToken"].as_str() {
            Some(token) if !page.is_empty() => Some(token.to_string()),
            _ => None,
        };
        if next_token.is_none() {
            break;
        }
    }
    assert!(
        next_token.is_none(),
        "case {case:?}: the enumeration did not end within the pages this store can fill"
    );
    entries
}

/// The page sizes a row of `held` instances is enumerated at.
///
/// Up to the output list's published maximum every size is used, including the request that
/// names none and one at the model's `MaxResults` ceiling, so an entry lands first, in the
/// middle and last on its page. Past that maximum only sizes at or below it are used: each
/// page then stays inside the list's own bound, and whether a listing larger than the bound
/// is itself clamped is a different rule from the view every served entry carries.
fn page_sizes(held: usize) -> Vec<Option<i64>> {
    if held > MAX_LISTED_INSTANCES {
        vec![Some(1), Some(3), Some(MAX_LISTED_INSTANCES as i64)]
    } else {
        vec![None, Some(1), Some(3), Some(100)]
    }
}

/// THE RULE over the cardinality of the enumeration and the position of an entry in it:
/// whatever the account holds and however the pages are cut, every entry served carries a
/// `State` structure whose `Name` is that entry's own `Status`.
#[tokio::test]
async fn every_entry_of_every_page_carries_its_own_state_view() {
    // `InstanceList` is `length min 0 max 10`: the empty listing, the sizes in between,
    // the published maximum and one past it.
    for held in [0, 1, 2, 5, MAX_LISTED_INSTANCES, MAX_LISTED_INSTANCES + 1] {
        for max_results in page_sizes(held) {
            let case = format!("{held} instance(s) / MaxResults {max_results:?}");
            let (router, expected) = seeded_account(held);
            let entries = enumerate(&case, &router, max_results, held).await;
            assert_eq!(
                entries.len(),
                held,
                "case {case:?}: expected every held instance to be enumerated once, got \
                 {entries:?}"
            );

            let mut seen = BTreeMap::new();
            for (position, entry) in entries.iter().enumerate() {
                let entry_case = format!("{case} / entry {position}");
                assert_state_mirrors_status(&entry_case, entry);
                let instance_arn = entry["InstanceArn"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {entry_case:?}: the entry reports no InstanceArn: {entry}")
                    })
                    .to_string();
                let state_name = entry["State"]["Name"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {entry_case:?}: the entry reports no State.Name: {entry}")
                    })
                    .to_string();
                seen.insert(instance_arn, state_name);
            }
            assert_eq!(
                seen, expected,
                "case {case:?}: expected each instance's State.Name to be its own stored \
                 status, got {seen:?}"
            );
        }
    }
}

/// The same rule where two instances carry ONE `Name`: each entry's `State` is its own, so
/// a listing in which a name picks out more than one entry still reports each instance's
/// lifecycle on the entry that instance was served as.
#[tokio::test]
async fn entries_sharing_one_name_each_report_their_own_state() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let now = SystemTime::now();
    // One `Name`, two instances, two different lifecycle values — the active instance and
    // one whose creation is still in flight.
    let shared_name = "OneNameTwoInstances";
    let rows = [
        ("ssoins-0b1c2d3e4f506172", "d-0a1b2c3d4e", "ACTIVE"),
        (
            "ssoins-7a8b9c0d1e2f3041",
            "d-5f6a7b8c9d",
            "CREATE_IN_PROGRESS",
        ),
    ];
    let expected: BTreeMap<String, String> = with_store(&engine, |store| {
        store.instances.clear();
        store.ambient_instance_enabled = true;
        let mut expected = BTreeMap::new();
        for (instance_id, identity_store_id, status) in rows {
            let mut instance = state::Instance::active(
                instance_id,
                identity_store_id,
                state::InstanceType::Account,
                Some(shared_name.to_string()),
                aws_smithy_types::DateTime::from(now),
                REGION,
            );
            instance.status = status.to_string();
            instance.active_due = None;
            instance.deleted_due = None;
            expected.insert(instance.instance_arn.clone(), status.to_string());
            store.instances.insert(instance.instance_arn.clone(), instance);
        }
        expected
    });
    let router = router_over(engine);

    let case = "two instances under one Name";
    let entries = enumerate(case, &router, None, rows.len()).await;
    assert_eq!(
        entries.len(),
        rows.len(),
        "case {case:?}: both instances are enumerated, got {entries:?}"
    );

    let mut seen = BTreeMap::new();
    for entry in &entries {
        assert_state_mirrors_status(case, entry);
        assert_eq!(
            entry["Name"].as_str(),
            Some(shared_name),
            "case {case:?}: both entries carry the shared Name, got {entry}"
        );
        seen.insert(
            entry["InstanceArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no InstanceArn in {entry}"))
                .to_string(),
            entry["State"]["Name"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no State.Name in {entry}"))
                .to_string(),
        );
    }
    assert_eq!(
        seen, expected,
        "case {case:?}: expected each entry's State.Name to be its own status, got {seen:?}"
    );
}
