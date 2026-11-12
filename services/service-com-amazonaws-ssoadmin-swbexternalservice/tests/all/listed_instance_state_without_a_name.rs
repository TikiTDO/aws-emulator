// SPDX-License-Identifier: CC0-1.0

//! The `State` view an enumerated IAM Identity Center instance carries when the entry has
//! NO `Name` member, over the `awsJson1_1` wire.
//!
//! THE RULE: the `State` structure an enumeration serves beside an entry's `Status` is read
//! from that entry's own stored lifecycle field and is keyed on nothing else the entry
//! reports, so an instance holding no `Name` at all is served with its view exactly like the
//! named instances beside it — `Name` is optional
//! (`smithy_query(query="shape",shape="com.amazonaws.ssoadmin#InstanceMetadata")` declares
//! it with no `@required`) and decides nothing about the projection.
//!
//! The sibling suites span the lifecycle (`instance_state_view.rs`), the record the view is
//! read from (`listed_instance_state_source.rs`), the `Name` VALUE space of the account's one
//! instance (`listed_instance_state_under_any_name.rs`), the write paths
//! (`listed_instance_state_after_instance_writes.rs`) and the SIZE of the listing
//! (`listed_instance_state_at_the_list_bound.rs`). This suite spans the one case none of them
//! reaches: an UNNAMED entry among named ones, at every position the unnamed entry can take
//! in the order the enumeration serves (first, middle, last — so no projection can be reading
//! the view off a neighbour), crossed with the lifecycle that entry is in (the
//! model-generated `ops::types::INSTANCE_STATUS_VALUES`) and the page size the request asks
//! for; plus the account that holds NO instance, the published minimum of the listing shape
//! (`com.amazonaws.ssoadmin#InstanceList`, `length 0..10`), whose enumeration serves no entry
//! and therefore none missing a view.
//!
//! Every call goes through `MultiServiceRouter::dispatch`; the harness is the one
//! `ssoadmin_permission_sets.rs` owns, and the enumeration and the rule assertion the ones
//! `instance_state_view.rs` owns. An account holds one instance and an organization instance
//! is enabled through AWS Organizations rather than through this API, so the records are
//! seeded through the service's own `state::Instance` constructor; everything asserted then
//! goes over the wire.

use std::sync::Arc;
use std::time::SystemTime;

use aws_smithy_types::DateTime;
use serde_json::Value;
use service_com_amazonaws_ssoadmin_swbexternalservice::ops::types::INSTANCE_STATUS_VALUES;
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::instance_state_view::{assert_state_mirrors_status, listed};
use crate::ssoadmin_permission_sets::{router_over, with_store};

/// The Region every instance below is enabled in, and the Region its calls are signed for: a
/// `RegionName` (`^([a-z]+-){2,3}\d$`) none of the sibling suites enumerates from.
const REGION: &str = "ap-south-1";

/// How many instances the account holds in the rows that mix a named with an unnamed entry:
/// enough for the unnamed one to be served first, in the middle, or last.
const HELD: usize = 3;

/// The status the NAMED instances carry, so the unnamed entry's own lifecycle is told apart
/// from theirs: a value of the model's `InstanceStatus` enum.
const NAMED_STATUS: &str = "ACTIVE";

/// A router over an account holding `HELD` instances, the one at `unnamed_position` carrying
/// no `Name` member and the status `unnamed_status`, every other carrying a name and
/// [`NAMED_STATUS`].
///
/// The ARNs are ordered by their index, and `ListInstances` enumerates by `InstanceArn`, so
/// `unnamed_position` IS the position the unnamed entry is served at.
fn account_holding(unnamed_position: usize, unnamed_status: &str) -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    with_store(&engine, |store| {
        store.instances.clear();
        // The account's enablement is settled, so no later request enables an ambient
        // instance beside the records seeded here.
        store.ambient_instance_enabled = true;
        for index in 0..HELD {
            let unnamed = index == unnamed_position;
            // `ssoins-` + 16 of `[a-zA-Z0-9-.]` and `d-` + 10 hex digits, as the model's
            // `InstanceArn` and `InstanceIdentityStoreArn` patterns require.
            let mut instance = state::Instance::active(
                &format!("ssoins-{:016x}", index + 1),
                &format!("d-{:010x}", index + 1),
                state::InstanceType::Organization,
                (!unnamed).then(|| format!("Named{index}")),
                DateTime::from(SystemTime::now()),
                REGION,
            );
            instance.status = if unnamed {
                unnamed_status.to_string()
            } else {
                NAMED_STATUS.to_string()
            };
            // Nothing is owed on the record: the status seeded here IS the instance's
            // status, so no transition the clock owes can move it out from under a read.
            instance.active_due = None;
            instance.deleted_due = None;
            store
                .instances
                .insert(instance.instance_arn.clone(), instance);
        }
    });
    router_over(engine)
}

/// Whether this entry reports no `Name` member at all — an absent member and an explicit
/// null both read as "this instance has no name".
fn reports_no_name(entry: &Value) -> bool {
    entry.get("Name").is_none_or(Value::is_null)
}

/// THE RULE: the unnamed entry carries its own `State` wherever in the enumeration it is
/// served and whatever lifecycle it is in, and so does every named entry beside it.
#[tokio::test]
async fn an_entry_with_no_name_carries_its_own_state() {
    for unnamed_position in 0..HELD {
        for unnamed_status in INSTANCE_STATUS_VALUES {
            let router = account_holding(unnamed_position, unnamed_status);
            // `MaxResults` is `range 1..100`: the page size that serves the entries one at a
            // time, one that splits them unevenly, and the request that names no size.
            for max_results in [Some(1), Some(2), None] {
                let case = format!(
                    "unnamed instance at position {unnamed_position} in {unnamed_status} / \
                     MaxResults {max_results:?}"
                );
                let entries = listed(&case, &router, REGION, max_results).await;
                assert_eq!(
                    entries.len(),
                    HELD,
                    "case {case:?}: every instance the account holds is enumerated, got \
                     {entries:?}"
                );

                let unnamed: Vec<&Value> = entries
                    .iter()
                    .filter(|entry| reports_no_name(entry))
                    .collect();
                assert_eq!(
                    unnamed.len(),
                    1,
                    "case {case:?}: exactly one held instance has no name, got {entries:?}"
                );
                assert_state_mirrors_status(&case, unnamed[0]);
                assert_eq!(
                    unnamed[0]["State"]["Name"].as_str(),
                    Some(*unnamed_status),
                    "case {case:?}: expected the unnamed entry to report State.Name \
                     {unnamed_status:?}, got {}",
                    unnamed[0]
                );
                assert_eq!(
                    entries
                        .iter()
                        .position(reports_no_name)
                        .expect("the unnamed entry was found above"),
                    unnamed_position,
                    "case {case:?}: the unnamed instance is served at the position its ARN \
                     orders it to, got {entries:?}"
                );

                for entry in entries.iter().filter(|entry| !reports_no_name(entry)) {
                    assert_state_mirrors_status(&case, entry);
                    assert_eq!(
                        entry["State"]["Name"].as_str(),
                        Some(NAMED_STATUS),
                        "case {case:?}: expected a named entry to report State.Name \
                         {NAMED_STATUS:?}, got {entry}"
                    );
                }
            }
        }
    }
}

/// THE RULE at the published MINIMUM of the listing shape: an account holding no instance is
/// enumerated as an empty listing, so there is no entry to carry a view and none missing one.
#[tokio::test]
async fn an_account_holding_no_instance_is_enumerated_empty() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    with_store(&engine, |store| {
        store.instances.clear();
        store.ambient_instance_enabled = true;
    });
    let router = router_over(engine);

    for max_results in [Some(1), Some(100), None] {
        let case = format!("no instance held / MaxResults {max_results:?}");
        let entries = listed(&case, &router, REGION, max_results).await;
        assert!(
            entries.is_empty(),
            "case {case:?}: an account holding no instance serves no entry, got {entries:?}"
        );
    }
}
