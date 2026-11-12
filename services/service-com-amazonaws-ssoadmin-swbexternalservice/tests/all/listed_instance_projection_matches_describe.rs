// SPDX-License-Identifier: CC0-1.0

//! The instance projection an enumeration serves is the one a single-instance read serves,
//! over the `awsJson1_1` wire.
//!
//! THE RULE: an instance view is ONE projection of one stored instance record, so every
//! entry `ListInstances` serves is the same `InstanceMetadata` structure — its `State`
//! view included, whose `Name` is that entry's own `Status` — that `DescribeInstance`
//! repeats in its `Instance` envelope for the same `InstanceArn`, whatever page the entry
//! arrives on and whatever page size the enumeration asked for.
//!
//! The sibling suites pin the view from the other side: `instance_state_view.rs` that the
//! member is served and mirrors the status through the lifecycle,
//! `listed_instance_state_source.rs` the record it is read from,
//! `listed_instance_state_under_any_name.rs` the `Name` value space, and
//! `instance_describe.rs` the FLAT members a describe shares with a listed entry. This
//! suite pins the two projections of one record against each other, member for member and
//! member SET for member set, so a member that is not in the on-disk model cannot be
//! served by one read and dropped by the other. The rows span what the rule generalises
//! over:
//!
//! * the lifecycle value the record holds — every value of the model-generated
//!   `ops::types::INSTANCE_STATUS_VALUES`, including the `CREATE_FAILED` no call of this
//!   API reaches;
//! * HOW MANY instances the account holds, so the enumeration spans more than one record;
//! * the page size: both published bounds of the operation's `MaxResults`
//!   (`smithy_query(query="operation",operation="ListInstances")`: `range 1..100`), a size
//!   between them, and the request that names none;
//! * an instance enabled over the WIRE by `CreateInstance` rather than seeded, read back
//!   while its creation is still in flight and once its activation has fallen due.
//!
//! Every call goes through `MultiServiceRouter::dispatch`; the harness is the one
//! `ssoadmin_permission_sets.rs` owns and the enumeration the one `instance_state_view.rs`
//! owns. Where a phase lives on the far side of a due instant the test moves the STORED
//! instant rather than sleeping — the transition is still applied by the service's own
//! `materialize_due`, exactly as in production.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::ops::types::INSTANCE_STATUS_VALUES;
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::instance_create::hold_activation;
use crate::instance_state_view::{assert_state_mirrors_status, listed};
use crate::ssoadmin_permission_sets::{call_in_region, router_over, with_store};

/// The Region every instance below is enabled in, and the Region its calls are signed for:
/// a `RegionName` (`^([a-z]+-){2,3}\d$`) none of the sibling suites uses.
const REGION: &str = "ap-south-1";

/// The page sizes the rows ask for: both published bounds of the operation's `MaxResults`
/// (`range 1..100`), a size between them, and the request that names none.
const PAGE_SIZES: [Option<i64>; 4] = [Some(1), Some(2), Some(100), None];

/// A router over an account that holds exactly `instances`, plus the engine behind it (for
/// the stored due instants a phase has to be moved past).
///
/// An account holds one instance, and an organization instance is enabled through AWS
/// Organizations rather than through this API, so a multi-instance account is seeded
/// through the service's own `state::Instance` constructors; everything asserted then goes
/// over the wire.
fn account_holding(
    instances: Vec<state::Instance>,
) -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    with_store(&engine, |store| {
        store.instances.clear();
        for instance in instances {
            store
                .instances
                .insert(instance.instance_arn.clone(), instance);
        }
        // The account's ambient instance has been enabled once already, so no later
        // request enables one beside the records seeded here.
        store.ambient_instance_enabled = true;
    });
    (router_over(Arc::clone(&engine)), engine)
}

/// One stored instance whose own status is `status`, with nothing owed on it: the status
/// seeded here IS the instance's status, so no transition the clock owes can move it out
/// from under either of the two reads the rule compares.
fn instance_in(
    instance_id: &str,
    identity_store_id: &str,
    name: &str,
    status: &str,
) -> state::Instance {
    let mut instance = state::Instance::active(
        instance_id,
        identity_store_id,
        state::InstanceType::Organization,
        Some(name.to_string()),
        DateTime::from(SystemTime::now()),
        REGION,
    );
    instance.status = status.to_string();
    instance.active_due = None;
    instance.deleted_due = None;
    instance
}

/// `DescribeInstance` for `instance_arn`, as a 200 body.
async fn describe(case: &str, router: &MultiServiceRouter, instance_arn: &str) -> Value {
    let (status, body) = call_in_region(
        router,
        REGION,
        "DescribeInstance",
        &json!({"InstanceArn": instance_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeInstance expected 200, got {status} {body}"
    );
    body
}

/// THE RULE on one served entry: it carries its `State` view, and the single-instance read
/// of the same `InstanceArn` serves that very projection — the same member set, the same
/// value under each member, and a flat `Status` that agrees with both.
async fn assert_one_projection(case: &str, router: &MultiServiceRouter, entry: &Value) {
    assert_state_mirrors_status(case, entry);
    let instance_arn = entry["InstanceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the entry reports no InstanceArn: {entry}"));

    let described = describe(case, router, instance_arn).await;
    let envelope = described.get("Instance").unwrap_or_else(|| {
        panic!("case {case:?}: the describe carries no Instance envelope: {described}")
    });

    let entry_members: Vec<&String> = entry
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: the entry is a structure, got {entry}"))
        .keys()
        .collect();
    let envelope_members: Vec<&String> = envelope
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: the envelope is a structure, got {envelope}"))
        .keys()
        .collect();
    assert_eq!(
        entry_members, envelope_members,
        "case {case:?}: the two projections of one record must report one member set, got \
         {entry_members:?} vs {envelope_members:?}"
    );
    assert!(
        entry_members.iter().any(|member| *member == "State"),
        "case {case:?}: the projection must carry the State member, got {entry_members:?}"
    );
    assert_eq!(
        envelope, entry,
        "case {case:?}: expected the listed entry and the described envelope to be one \
         projection, got {envelope} vs {entry}"
    );
    assert_eq!(
        described["Status"],
        entry["State"]["Name"],
        "case {case:?}: the described Status and the listed State.Name report one \
         lifecycle, got {} vs {}",
        described["Status"],
        entry["State"]["Name"]
    );
}

/// THE RULE over an account holding several instances in different lifecycle values, at
/// every page size the rows span: each entry, on whichever page it arrives, is the
/// projection the single-instance read of its own ARN serves.
#[tokio::test]
async fn every_listed_entry_is_the_projection_the_single_instance_read_serves() {
    // One instance per value the model's enum declares, so nothing here is a hand-written
    // catalog: the account holds as many records as there are lifecycle values, and the
    // enumeration therefore spans more than one record at every page size below.
    let instances: Vec<state::Instance> = INSTANCE_STATUS_VALUES
        .iter()
        .enumerate()
        .map(|(index, status)| {
            let digit = char::from(b'1' + index as u8).to_string();
            instance_in(
                &format!("ssoins-{}", digit.repeat(16)),
                &format!("d-{}", digit.repeat(10)),
                &format!("Instance{index}"),
                status,
            )
        })
        .collect();
    let expected_statuses: Vec<String> = {
        let mut statuses: Vec<String> = INSTANCE_STATUS_VALUES
            .iter()
            .map(|status| (*status).to_string())
            .collect();
        statuses.sort();
        statuses
    };
    let (router, _engine) = account_holding(instances);

    for max_results in PAGE_SIZES {
        let case = format!(
            "{} instances / MaxResults {max_results:?}",
            INSTANCE_STATUS_VALUES.len()
        );
        let entries = listed(&case, &router, REGION, max_results).await;
        assert_eq!(
            entries.len(),
            INSTANCE_STATUS_VALUES.len(),
            "case {case:?}: every instance the account holds is enumerated, got {entries:?}"
        );

        let mut seen = Vec::new();
        for entry in &entries {
            assert_one_projection(&case, &router, entry).await;
            seen.push(
                entry["State"]["Name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        seen.sort();
        assert_eq!(
            seen, expected_statuses,
            "case {case:?}: each entry reports its own stored lifecycle, got {seen:?}"
        );
    }
}

/// The same rule for an instance enabled over the WIRE: the enumeration that follows the
/// create serves the instance with its `State` view while the creation is still in flight
/// and once the activation the clock owes has fallen due, and each time the entry is the
/// projection the describe of the created ARN serves.
///
/// The `Name` is generated per run, as a caller's own set-up generates one, so nothing the
/// view reports may be keyed on a name the service did not choose.
#[tokio::test]
async fn an_instance_enabled_over_the_wire_is_listed_as_the_describe_reports_it() {
    let (router, engine) = account_holding(Vec::new());
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a clock at or after the Unix epoch")
        .subsec_nanos();
    // `NameType`: `length 0..255`, `pattern ^[\w+=,.@-]+$` — a prefix and a hex suffix.
    let name = format!("wire-idc-{suffix:08x}");

    let (status, created) =
        call_in_region(&router, REGION, "CreateInstance", &json!({"Name": name})).await;
    assert_eq!(
        status, 200,
        "CreateInstance expected 200, got {status} {created}"
    );
    let instance_arn = created["InstanceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no InstanceArn in {created}"))
        .to_string();

    // PHASE 1 — the creation is still in flight, so the entry reports the transient value;
    // PHASE 2 — the activation the clock owes has fallen due.
    for (phase, due_at, expected_status) in [
        (
            "creation in flight",
            SystemTime::now() + Duration::from_secs(3_600),
            "CREATE_IN_PROGRESS",
        ),
        (
            "activation due",
            SystemTime::now() - Duration::from_secs(1),
            "ACTIVE",
        ),
    ] {
        hold_activation(&engine, due_at);
        let case = format!("instance enabled over the wire / {phase}");
        let entries = listed(&case, &router, REGION, None).await;
        assert_eq!(
            entries.len(),
            1,
            "case {case:?}: the account's one instance is enumerated, got {entries:?}"
        );
        let entry = &entries[0];
        assert_eq!(
            entry["InstanceArn"].as_str(),
            Some(instance_arn.as_str()),
            "case {case:?}: expected the created instance, got {entry}"
        );
        assert_eq!(
            entry["Name"].as_str(),
            Some(name.as_str()),
            "case {case:?}: expected the generated Name {name:?}, got {entry}"
        );
        assert_eq!(
            entry["State"]["Name"].as_str(),
            Some(expected_status),
            "case {case:?}: expected State.Name {expected_status:?}, got {entry}"
        );
        assert_one_projection(&case, &router, entry).await;
    }
}
