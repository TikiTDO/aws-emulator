// SPDX-License-Identifier: CC0-1.0

//! The `State` view an enumerated IAM Identity Center instance carries, over the
//! `awsJson1_1` wire.
//!
//! THE RULE: every instance an enumeration serves carries, beside its `Status`, a `State`
//! structure whose `Name` is that same lifecycle value — read from the instance being
//! served, so the two views of one instance's lifecycle can never disagree, whichever
//! value of the model's `InstanceStatus` enum the instance is in
//! (`smithy_query(query="shape",shape="InstanceStatus")`: `CREATE_IN_PROGRESS |
//! CREATE_FAILED | DELETE_IN_PROGRESS | ACTIVE`). The structure behind `State` is not in
//! the on-disk model, so the member list it reports is the single `Name` the service
//! serves.
//!
//! The rows span what the rule generalises over: the lifecycle PHASE the instance is in
//! (creation in flight, activation due, deletion in flight, and the `CREATE_FAILED` value
//! no call of this API reaches), the KIND of instance (the account's ambient instance, an
//! instance `CreateInstance` minted, an organization instance), the Region the instance is
//! enabled in, HOW MANY instances the enumeration serves at once — two instances in
//! different phases, so the mapping is per entry rather than one constant for the page —
//! and the PAGE the entry arrives on. The admissible status values are not written out by
//! hand: they are the model-generated `ops::types::INSTANCE_STATUS_VALUES`.
//!
//! Every call goes through `MultiServiceRouter::dispatch`; the harness is the one
//! `ssoadmin_permission_sets.rs` owns. Where a phase lives on the far side of a due
//! instant the test moves the STORED instant rather than sleeping — the transition is
//! still applied by the service's own `materialize_due`, exactly as in production.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::ops::types::INSTANCE_STATUS_VALUES;
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::instance_create::hold_activation;
use crate::ssoadmin_permission_sets::{call_in_region, hold_deletion, router_over, with_store};

/// A router over an account that holds NO instance yet, so `CreateInstance` is the call
/// that enables the account's one instance — and the engine behind it, for the stored due
/// instants a phase has to be moved past.
fn empty_account() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    with_store(&engine, |store| {
        store.instances.clear();
        store.ambient_instance_enabled = true;
    });
    (router_over(Arc::clone(&engine)), engine)
}

/// A router over an account whose instance is the AMBIENT one — the instance a client
/// discovers with no create first — and the engine behind it.
fn ambient_account() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (router_over(Arc::clone(&engine)), engine)
}

/// The entries `ListInstances` serves in `region`, following every `NextToken` it hands
/// out so an entry is asserted on whichever page it arrives on.
///
/// Shared with the suite that pins WHERE the `State` view is read from
/// (`listed_instance_state_source.rs`), so both drive one enumeration helper.
pub(crate) async fn listed(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    max_results: Option<i64>,
) -> Vec<Value> {
    // One more request than there can be pages: the enumeration ends on the page that
    // returns nothing, and the store holds at most a handful of instances.
    listed_pages(case, router, region, max_results, 8)
        .await
        .concat()
}

/// The same enumeration, PAGE BY PAGE: the entries each request served, in the order the
/// requests were made, for a suite whose subject is how a listing is cut up rather than
/// which entries it serves.
///
/// `requests` bounds how many requests the walk makes — the enumeration ends on the page
/// that returns nothing, so a caller passes one more than the store can fill at the
/// smallest page the model admits. Shared with
/// `listed_instance_state_generated_accounts.rs`, whose populations are larger than the
/// handful [`listed`] walks.
pub(crate) async fn listed_pages(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    max_results: Option<i64>,
    requests: usize,
) -> Vec<Vec<Value>> {
    let mut pages = Vec::new();
    let mut next_token: Option<String> = None;
    for _ in 0..requests {
        let mut request = json!({});
        if let Some(max) = max_results {
            request["MaxResults"] = json!(max);
        }
        if let Some(token) = next_token.as_deref() {
            request["NextToken"] = json!(token);
        }
        let (status, body) = call_in_region(router, region, "ListInstances", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListInstances expected 200, got {status} {body}"
        );
        let page = body["Instances"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no Instances in {body}"))
            .clone();
        let empty = page.is_empty();
        pages.push(page);
        next_token = match body["NextToken"].as_str() {
            Some(token) if !empty => Some(token.to_string()),
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
    pages
}

/// THE RULE, asserted on one served entry: its `State` is a structure whose `Name` is the
/// entry's own `Status`, and that status is a value the model's enum declares.
///
/// Shared with `listed_instance_state_source.rs`, which asserts the same rule over the
/// records a `State` could wrongly be read from.
pub(crate) fn assert_state_mirrors_status(case: &str, entry: &Value) {
    let status = entry["Status"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the entry reports no Status: {entry}"));
    assert!(
        INSTANCE_STATUS_VALUES.contains(&status),
        "case {case:?}: expected one of the modeled InstanceStatus values \
         {INSTANCE_STATUS_VALUES:?}, got {status:?}"
    );
    let state = entry.get("State").unwrap_or_else(|| {
        panic!("case {case:?}: the entry carries no State member at all: {entry}")
    });
    let members = state.as_object().unwrap_or_else(|| {
        panic!("case {case:?}: State is a structure, got {state:?} in {entry}")
    });
    assert_eq!(
        state["Name"].as_str(),
        Some(status),
        "case {case:?}: expected State.Name {status:?}, got {:?}",
        state["Name"]
    );
    assert_eq!(
        members.keys().collect::<Vec<_>>(),
        vec!["Name"],
        "case {case:?}: expected State to carry only Name, got {members:?}"
    );
}

/// THE RULE over the instance lifecycle, for an instance `CreateInstance` minted: every
/// phase the instance passes through is reported by `Status` and by `State.Name` alike,
/// in every Region an instance can be enabled in.
#[tokio::test]
async fn a_listed_instance_carries_its_status_as_its_state_through_the_lifecycle() {
    // The Region the instance is enabled in: the model's `RegionName` pattern
    // `^([a-z]+-){2,3}\d$` in two- and three-prefix spellings, across partitions. None of
    // them is the Region the recorded case used.
    let regions = ["us-east-1", "eu-central-1", "ap-southeast-3", "cn-north-1"];

    for region in regions {
        let (router, engine) = empty_account();
        let (status, created) = call_in_region(
            &router,
            region,
            "CreateInstance",
            &json!({"Name": "StateView"}),
        )
        .await;
        assert_eq!(
            status, 200,
            "region {region}: CreateInstance expected 200, got {status} {created}"
        );
        let instance_arn = created["InstanceArn"]
            .as_str()
            .unwrap_or_else(|| panic!("region {region}: no InstanceArn in {created}"))
            .to_string();

        // PHASE 1 — the creation is in flight: CREATE_IN_PROGRESS.
        hold_activation(&engine, SystemTime::now() + Duration::from_secs(3_600));
        let case = format!("{region} / creation in flight");
        for entry in listed(&case, &router, region, None).await {
            assert_state_mirrors_status(&case, &entry);
            assert_eq!(
                entry["Status"].as_str(),
                Some("CREATE_IN_PROGRESS"),
                "case {case:?}: the creation is still in flight, got {entry}"
            );
        }

        // PHASE 2 — the activation has fallen due: ACTIVE, and it stays ACTIVE across
        // repeated reads.
        hold_activation(&engine, SystemTime::now() - Duration::from_secs(1));
        for read in 0..2 {
            let case = format!("{region} / activation due / read {read}");
            for entry in listed(&case, &router, region, None).await {
                assert_state_mirrors_status(&case, &entry);
                assert_eq!(
                    entry["Status"].as_str(),
                    Some("ACTIVE"),
                    "case {case:?}: the activation was due, got {entry}"
                );
            }
        }

        // PHASE 3 — a deletion is in flight: DELETE_IN_PROGRESS until the removal falls
        // due.
        let (status, deleted) = call_in_region(
            &router,
            region,
            "DeleteInstance",
            &json!({"InstanceArn": instance_arn}),
        )
        .await;
        assert_eq!(
            status, 200,
            "region {region}: DeleteInstance expected 200, got {status} {deleted}"
        );
        hold_deletion(&engine, SystemTime::now() + Duration::from_secs(3_600));
        let case = format!("{region} / deletion in flight");
        let entries = listed(&case, &router, region, None).await;
        assert_eq!(
            entries.len(),
            1,
            "case {case:?}: the instance being deleted is still enumerated, got {entries:?}"
        );
        for entry in entries {
            assert_state_mirrors_status(&case, &entry);
            assert_eq!(
                entry["Status"].as_str(),
                Some("DELETE_IN_PROGRESS"),
                "case {case:?}: the instance is being deleted, got {entry}"
            );
        }
    }
}

/// THE RULE on the account's AMBIENT instance — the one a client discovers with no create
/// first — and on the fourth `InstanceStatus` value, `CREATE_FAILED`, which no call of
/// this API reaches: the mirroring is read off the stored status whatever put it there,
/// so every value the enum declares is covered.
#[tokio::test]
async fn the_state_of_a_discovered_instance_is_its_stored_status() {
    // Every value the model's enum declares, so nothing here is a hand-written catalog:
    // the two a wire call can leave are driven over the wire above, and the row below
    // seeds each of them (`CREATE_FAILED` included) on the ambient instance.
    for status in INSTANCE_STATUS_VALUES {
        let (router, engine) = ambient_account();
        // Touch the store through a call first, so the ambient instance is enabled by the
        // service rather than by this test, and only its STATUS is then seeded.
        let case = format!("ambient instance in {status}");
        let discovered = listed(&case, &router, "us-west-2", None).await;
        assert_eq!(
            discovered.len(),
            1,
            "case {case:?}: an account discovers exactly its ambient instance, got \
             {discovered:?}"
        );
        for entry in &discovered {
            assert_state_mirrors_status(&case, entry);
        }

        with_store(&engine, |store| {
            for instance in store.instances.values_mut() {
                instance.status = (*status).to_string();
                // Nothing is owed: the status seeded here IS the instance's status, so no
                // transition the clock owes can move it out from under the read.
                instance.active_due = None;
                instance.deleted_due = None;
            }
        });

        for entry in listed(&case, &router, "us-west-2", None).await {
            assert_state_mirrors_status(&case, &entry);
            assert_eq!(
                entry["Status"].as_str(),
                Some(*status),
                "case {case:?}: expected the stored status, got {entry}"
            );
        }
    }
}

/// THE RULE where the enumeration serves MORE than one instance, each in a different
/// phase, over every page size the model's `MaxResults` admits for this page: each entry's
/// `State.Name` is its OWN status, so no page reports one instance's lifecycle for
/// another's.
#[tokio::test]
async fn each_entry_of_a_page_mirrors_its_own_status() {
    let region = "ap-northeast-1";
    let (router, engine) = empty_account();
    // Two instances, seeded through the service's own constructors (an account holds one
    // instance, so no call of this API enables a second), in two different phases: one
    // ACTIVE and one whose creation is still in flight. Everything asserted then goes
    // over the wire.
    let now = SystemTime::now();
    with_store(&engine, |store| {
        let active = state::Instance::active(
            "ssoins-1111111111111111",
            "d-1111111111",
            state::InstanceType::Organization,
            Some("Active".to_string()),
            aws_smithy_types::DateTime::from(now),
            region,
        );
        let creating = state::Instance::creating(
            "ssoins-2222222222222222",
            "d-2222222222",
            state::InstanceType::Account,
            Some("Creating".to_string()),
            // Far enough out that the activation is not owed while this test reads.
            now + Duration::from_secs(3_600),
            region,
        );
        store
            .instances
            .insert(active.instance_arn.clone(), active);
        store
            .instances
            .insert(creating.instance_arn.clone(), creating);
        store.ambient_instance_enabled = true;
    });

    // `MaxResults` is `range 1..100`, so page sizes 1 and 2 split the two instances across
    // pages and serve them on one page; `None` is the request that names no page size.
    for max_results in [Some(1), Some(2), Some(100), None] {
        let case = format!("two instances / MaxResults {max_results:?}");
        let entries = listed(&case, &router, region, max_results).await;
        assert_eq!(
            entries.len(),
            2,
            "case {case:?}: both instances are enumerated, got {entries:?}"
        );
        let mut seen = Vec::new();
        for entry in &entries {
            assert_state_mirrors_status(&case, entry);
            seen.push((
                entry["Name"].as_str().unwrap_or_default().to_string(),
                entry["State"]["Name"].as_str().unwrap_or_default().to_string(),
            ));
        }
        seen.sort();
        assert_eq!(
            seen,
            vec![
                ("Active".to_string(), "ACTIVE".to_string()),
                (
                    "Creating".to_string(),
                    "CREATE_IN_PROGRESS".to_string()
                ),
            ],
            "case {case:?}: each entry reports its own lifecycle, got {seen:?}"
        );
    }
}
