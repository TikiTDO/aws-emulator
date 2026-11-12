// SPDX-License-Identifier: CC0-1.0

//! The `State` view every enumerated IAM Identity Center instance carries when the
//! enumeration serves MANY instances at once, over the `awsJson1_1` wire.
//!
//! THE RULE: the instance projection an enumeration serves is built per instance from that
//! instance's own stored record, so EVERY entry of EVERY page carries a `State` structure
//! whose `Name` is that entry's own `Status` — however many instances the account holds and
//! whatever page size the request asks for, since neither the size of the listing nor the
//! page an entry arrives on is part of the projection.
//!
//! `instance_state_view.rs` pins that the member is served and mirrors the status through
//! the lifecycle, `listed_instance_state_source.rs` the record it is read from,
//! `listed_instance_state_under_any_name.rs` the `Name` value space and
//! `listed_instance_state_after_instance_writes.rs` the write paths. This suite spans the
//! COUNT of instances served — up to the published maximum of the listing's own shape
//! (`smithy_query(query="shape",shape="com.amazonaws.ssoadmin#InstanceList")`: `list`,
//! `length 0..10`) — crossed with the page size the request asks for (the published bounds
//! and interior values of `MaxResults`, `range 1..100`, and the request that asks for no
//! size at all), with the entries' statuses drawn from the model-generated
//! `ops::types::INSTANCE_STATUS_VALUES` so that one page carries SEVERAL different
//! lifecycles at once: a projection that served one record's view for a whole page, or
//! dropped the member once a page was filled, could not satisfy these rows.
//!
//! Every call goes through `MultiServiceRouter::dispatch`; the harness is the one
//! `ssoadmin_permission_sets.rs` owns and the rule assertion the one
//! `instance_state_view.rs` owns. The enumeration is walked here rather than through that
//! suite's helper because a ten-instance account paged one entry at a time needs more
//! requests than it allows for. An account holds one instance, so the records are seeded
//! through the service's own `state::Instance` constructor — as the sibling suites do — and
//! everything asserted then goes over the wire.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::ops::types::INSTANCE_STATUS_VALUES;
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::instance_state_view::assert_state_mirrors_status;
use crate::ssoadmin_permission_sets::{call_in_region, router_over, with_store};

/// The Region every instance below is enabled in, and the Region its calls are signed for:
/// a `RegionName` (`^([a-z]+-){2,3}\d$`) none of the sibling suites enumerates from.
const REGION: &str = "us-west-1";

/// The published maximum of the listing shape `ListInstances` serves
/// (`com.amazonaws.ssoadmin#InstanceList`, `length 0..10`), which is the largest number of
/// entries one page can carry.
const INSTANCE_LIST_MAX: usize = 10;

/// An account holding `count` instances, each with its own identity and its own status —
/// the router over it, the engine behind it, and the status each instance is seeded with
/// keyed by `InstanceArn`.
///
/// The statuses are taken from the model's own `InstanceStatus` value list and advance by
/// THREE per instance, so neither the order the entries are served in nor the page they
/// land on lines up with the order of the value list: an entry's view has to come from its
/// own record.
fn account_holding(
    count: usize,
) -> (
    MultiServiceRouter,
    Arc<engine::InMemoryEngine>,
    BTreeMap<String, String>,
) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut expected = BTreeMap::new();
    with_store(&engine, |store| {
        store.instances.clear();
        // The account's ambient instance has been enabled once already, so no later request
        // enables one beside the records seeded here.
        store.ambient_instance_enabled = true;
        for index in 0..count {
            // `ssoins-` + 16 of `[a-zA-Z0-9-.]` and `d-` + 10 hex digits, as the model's
            // `InstanceArn` and `InstanceIdentityStoreArn` patterns require.
            let instance_id = format!("ssoins-{:016x}", index + 1);
            let identity_store_id = format!("d-{:010x}", index + 1);
            let mut instance = state::Instance::active(
                &instance_id,
                &identity_store_id,
                state::InstanceType::Organization,
                Some(format!("AtTheBound{index}")),
                DateTime::from(SystemTime::now()),
                REGION,
            );
            let status = INSTANCE_STATUS_VALUES[(index * 3) % INSTANCE_STATUS_VALUES.len()];
            instance.status = status.to_string();
            // Nothing is owed on the record: the status seeded here IS the instance's
            // status, so no transition the clock owes can move it out from under a read.
            instance.active_due = None;
            instance.deleted_due = None;
            expected.insert(instance.instance_arn.clone(), status.to_string());
            store
                .instances
                .insert(instance.instance_arn.clone(), instance);
        }
    });
    (router_over(Arc::clone(&engine)), engine, expected)
}

/// Seed each held instance with the status `statuses` gives for its `InstanceArn`, leaving
/// nothing owed on the record, so the lifecycle a read reports is the one seeded here.
fn seed_statuses(engine: &Arc<engine::InMemoryEngine>, statuses: &BTreeMap<String, String>) {
    with_store(engine, |store| {
        for (instance_arn, status) in statuses {
            let instance = store
                .instance_mut(instance_arn)
                .unwrap_or_else(|| panic!("no instance is held under {instance_arn}"));
            instance.status = status.clone();
            instance.active_due = None;
            instance.deleted_due = None;
        }
    });
}

/// Walk `ListInstances` to exhaustion at `max_results`, returning every entry served and
/// the size of each page.
///
/// The enumeration ends on the page that returns nothing, so a listing of `n` instances
/// paged one entry at a time takes `n + 1` requests; the bound below is generous enough for
/// the largest listing this suite builds and still terminates a walk that does not end.
async fn walk(
    case: &str,
    router: &MultiServiceRouter,
    max_results: Option<i64>,
) -> (Vec<Value>, Vec<usize>) {
    let mut entries = Vec::new();
    let mut page_sizes = Vec::new();
    let mut next_token: Option<String> = None;
    for _ in 0..(INSTANCE_LIST_MAX + 4) {
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
        page_sizes.push(page.len());
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
    (entries, page_sizes)
}

/// THE RULE, asserted over every entry an enumeration served: each one carries its `State`
/// view, that view's `Name` is the status the entry's OWN record holds, and the entries
/// together are exactly the instances the account holds.
fn assert_every_entry_reports_its_own_state(
    case: &str,
    entries: &[Value],
    expected: &BTreeMap<String, String>,
) {
    let mut served: BTreeMap<String, String> = BTreeMap::new();
    for entry in entries {
        assert_state_mirrors_status(case, entry);
        let instance_arn = entry["InstanceArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the entry reports no InstanceArn: {entry}"));
        let status = expected.get(instance_arn).unwrap_or_else(|| {
            panic!("case {case:?}: {instance_arn} is not an instance the account holds: {entry}")
        });
        assert_eq!(
            entry["State"]["Name"].as_str(),
            Some(status.as_str()),
            "case {case:?}: expected {instance_arn} to report State.Name {status:?}, got {entry}"
        );
        let previous = served.insert(
            instance_arn.to_string(),
            entry["State"]["Name"].as_str().unwrap_or_default().to_string(),
        );
        assert!(
            previous.is_none(),
            "case {case:?}: {instance_arn} was served twice by one enumeration"
        );
    }
    assert_eq!(
        &served, expected,
        "case {case:?}: every instance the account holds is served with its own State, got \
         {served:?}"
    );
}

/// THE RULE over the SIZE of the listing and the size of a page: from one instance up to the
/// published maximum of the listing shape, at every page size the model's `MaxResults`
/// admits, every entry of every page carries its own `State`.
#[tokio::test]
async fn every_entry_of_every_page_carries_its_own_state() {
    // The number of instances the account holds: one, a pair, an interior value, and the
    // published maximum of `InstanceList`.
    for count in [1usize, 2, 7, INSTANCE_LIST_MAX] {
        let (router, _engine, expected) = account_holding(count);
        // `MaxResults` is `range 1..100`: its published bounds, two interior sizes that
        // split the listing across pages, and the request that asks for no size at all.
        for max_results in [Some(1), Some(3), Some(10), Some(100), None] {
            let case = format!("{count} instances / MaxResults {max_results:?}");
            let (entries, page_sizes) = walk(&case, &router, max_results).await;
            assert_eq!(
                entries.len(),
                count,
                "case {case:?}: every instance the account holds is enumerated, got \
                 {entries:?}"
            );
            assert_every_entry_reports_its_own_state(&case, &entries, &expected);
            // No page carries more entries than the listing shape admits, nor more than the
            // request asked for: the rule is about entries the service DID serve, so the
            // pages they arrived on are worth naming when one of them is wrong.
            for (page, size) in page_sizes.iter().enumerate() {
                assert!(
                    *size <= INSTANCE_LIST_MAX,
                    "case {case:?}: page {page} served {size} entries, past the {INSTANCE_LIST_MAX} \
                     the listing shape admits"
                );
                if let Some(max) = max_results {
                    assert!(
                        *size <= max as usize,
                        "case {case:?}: page {page} served {size} entries for MaxResults {max}"
                    );
                }
            }
        }
    }
}

/// THE RULE where all but ONE of the instances share a lifecycle: the odd instance's entry
/// reports its own `State`, and so does every other, so a projection that served one
/// record's view for a whole page — the majority value, the first entry's, or the one the
/// page began with — cannot satisfy the row.
#[tokio::test]
async fn one_instance_in_another_phase_keeps_its_own_state() {
    for odd_status in INSTANCE_STATUS_VALUES {
        for majority_status in INSTANCE_STATUS_VALUES {
            if odd_status == majority_status {
                // The row is a test of a MIXED page; where the two values are the same
                // there is no odd instance to tell apart.
                continue;
            }
            let (router, engine, mut expected) = account_holding(INSTANCE_LIST_MAX);
            // The shared lifecycle on every instance, with the LAST one in ARN order — the
            // entry a one-entry page size serves last — in the odd lifecycle.
            let arns: Vec<String> = expected.keys().cloned().collect();
            let odd_arn = arns
                .last()
                .expect("the account holds the instances seeded above")
                .clone();
            for arn in &arns {
                let status = if arn == &odd_arn {
                    odd_status
                } else {
                    majority_status
                };
                expected.insert(arn.clone(), (*status).to_string());
            }

            let case = format!("{majority_status} majority / one {odd_status}");
            seed_statuses(&engine, &expected);
            for max_results in [Some(1), Some(100), None] {
                let case = format!("{case} / MaxResults {max_results:?}");
                let (entries, _) = walk(&case, &router, max_results).await;
                assert_eq!(
                    entries.len(),
                    INSTANCE_LIST_MAX,
                    "case {case:?}: every instance the account holds is enumerated, got \
                     {entries:?}"
                );
                assert_every_entry_reports_its_own_state(&case, &entries, &expected);
            }
        }
    }
}
