// SPDX-License-Identifier: CC0-1.0

//! The `State` view an IAM Identity Center instance carries at EVERY step of a sequence of
//! interleaved reads and writes, over the `awsJson1_1` wire.
//!
//! THE RULE: the instance projection this service serves is built from the stored instance
//! record as it stands at the instant of the read, so every entry of every `ListInstances`
//! page — and the `Instance` envelope `DescribeInstance` repeats — carries a `State`
//! structure whose single `Name` member is that record's own `Status`, for as long as the
//! record exists and whatever calls ran before this one.
//!
//! The sibling suites each isolate ONE call before one enumeration:
//! `instance_state_view.rs` pins that the member is served and mirrors the status through
//! the lifecycle, `listed_instance_state_after_instance_writes.rs` one write path,
//! `listed_instance_state_across_read_edge_cases.rs` one read path,
//! `listed_instance_state_source.rs` the record the view is read from and
//! `listed_instance_state_generated_accounts.rs` the same rule over drawn populations. The
//! axis this suite adds is the LENGTH and ORDER of the call SEQUENCE: each row is a script
//! of reads and writes a probing client makes in one order, the view is asserted after
//! EVERY step of it, and the scripts differ in how many steps they take, which call comes
//! first, which write falls between two reads, the page size each read asks for and whether
//! a deletion moves the expected lifecycle value mid-script. A projection that serves the
//! member only until some other call has run, or only on the first read after a write,
//! fails here.
//!
//! The page sizes are inside `MaxResults`' published range
//! (`smithy_query(query="operation",operation="ListInstances")`: `range 1..100`), plus the
//! request that names none; every `Name` a rename writes is a word of `NameType`'s
//! `^[\w+=,.@-]+$`.
//!
//! Every call goes through `MultiServiceRouter::dispatch`; the harness is the one
//! `ssoadmin_permission_sets.rs` owns, the enumeration and the rule assertion the ones
//! `instance_state_view.rs` owns, and the empty account and the held activation instant the
//! ones `instance_create.rs` owns. Where a phase lives on the far side of a due instant the
//! test moves the STORED instant rather than sleeping — the transition is still applied by
//! the service's own `materialize_due`, exactly as in production.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state::{
    STATUS_ACTIVE, STATUS_DELETE_IN_PROGRESS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::instance_create::{empty_account_router, hold_activation};
use crate::instance_state_view::{assert_state_mirrors_status, listed};
use crate::ssoadmin_permission_sets::{hold_deletion, served_in_region};

/// The Region the instance is enabled in and every call below is signed for: a `RegionName`
/// (`^([a-z]+-){2,3}\d$`) none of the sibling suites drives.
const REGION: &str = "eu-north-1";

/// The `Name` the instance is created with — the shape of a probing client's generated
/// name, a prefix and a hex suffix the caller minted, so nothing about the view can be
/// keyed on a name the service chose.
const CREATED_NAME: &str = "sequence-idc-5a3c91e0";

/// One step of a script: a call the client makes on the instance surface.
#[derive(Debug, Clone, Copy)]
enum Step {
    /// `ListInstances` at this page size (`None` names none), continued through every
    /// `NextToken` it hands out.
    Enumerate(Option<i64>),
    /// `UpdateInstance` renaming the instance to this `Name`.
    Rename(&'static str),
    /// `TagResource` writing this key on the instance itself.
    Tag(&'static str),
    /// `DeleteInstance`, with the removal the clock owes held open, so the record is still
    /// served in the status the delete left it in.
    StartDeletion,
}

/// One row: a sequence of steps, in the order the client makes them.
struct Script {
    label: &'static str,
    steps: &'static [Step],
}

/// The scripts the rows span — differing in length, in which call comes first, in which
/// write falls between two reads, in the page size each read asks for and in whether the
/// expected lifecycle value changes mid-script.
const SCRIPTS: [Script; 5] = [
    Script {
        label: "a read, then a write, then a read",
        steps: &[
            Step::Enumerate(None),
            Step::Rename("RenamedOnce"),
            Step::Enumerate(None),
        ],
    },
    Script {
        label: "two renames before the first enumeration",
        steps: &[
            Step::Rename("RenamedFirst"),
            Step::Rename("RenamedSecond"),
            Step::Enumerate(Some(1)),
        ],
    },
    Script {
        label: "writes and reads interleaved across page sizes",
        steps: &[
            Step::Enumerate(Some(1)),
            Step::Rename("Renamed.Mid-1"),
            Step::Enumerate(Some(100)),
            Step::Tag("Owner"),
            Step::Enumerate(None),
        ],
    },
    Script {
        label: "a deletion in the middle of the sequence",
        steps: &[
            Step::Rename("RenamedBeforeDelete"),
            Step::Enumerate(None),
            Step::StartDeletion,
            Step::Enumerate(Some(1)),
            Step::Enumerate(None),
        ],
    },
    Script {
        label: "reads only, repeated",
        steps: &[
            Step::Enumerate(None),
            Step::Enumerate(Some(2)),
            Step::Enumerate(None),
        ],
    },
];

/// What the record holds after the steps taken so far: the lifecycle value both reads must
/// report, and the `Name` the last rename wrote.
#[derive(Debug, Clone)]
struct Expected {
    status: &'static str,
    name: String,
}

/// One call made in [`REGION`], asserted served — the harness's own
/// [`served_in_region`] bound to this suite's Region.
async fn served(case: &str, router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    served_in_region(case, router, REGION, operation, body).await
}

/// `CreateInstance` on an account that holds none, as the created instance's ARN.
async fn create(case: &str, router: &MultiServiceRouter) -> String {
    let created = served(case, router, "CreateInstance", &json!({"Name": CREATED_NAME})).await;
    created["InstanceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no InstanceArn in {created}"))
        .to_string()
}

/// THE RULE, asserted on both reads of the record: the listed entry and the `Instance`
/// envelope each carry a `State` structure whose `Name` is the status the record holds, and
/// the two reads report ONE lifecycle — so a client that waits on `State` through either
/// call sees the same answer at this step.
async fn assert_views(
    case: &str,
    router: &MultiServiceRouter,
    instance_arn: &str,
    expected: &Expected,
    max_results: Option<i64>,
) {
    let entries = listed(case, router, REGION, max_results).await;
    assert_eq!(
        entries.len(),
        1,
        "case {case:?}: the account's one instance is enumerated, got {entries:?}"
    );
    let entry = &entries[0];
    assert_eq!(
        entry["InstanceArn"].as_str(),
        Some(instance_arn),
        "case {case:?}: expected the created instance, got {entry}"
    );
    assert_state_mirrors_status(case, entry);
    assert_eq!(
        entry["Status"].as_str(),
        Some(expected.status),
        "case {case:?}: expected Status {:?}, got {entry}",
        expected.status
    );
    assert_eq!(
        entry["State"]["Name"].as_str(),
        Some(expected.status),
        "case {case:?}: expected State.Name {:?}, got {entry}",
        expected.status
    );
    // The projection is the record's, so the `Name` the last write stored is read back
    // beside the `State` the same record supplies.
    assert_eq!(
        entry["Name"].as_str(),
        Some(expected.name.as_str()),
        "case {case:?}: expected Name {:?}, got {entry}",
        expected.name
    );

    let described = served(
        case,
        router,
        "DescribeInstance",
        &json!({"InstanceArn": instance_arn}),
    )
    .await;
    assert_eq!(
        described["Status"].as_str(),
        Some(expected.status),
        "case {case:?}: expected the described Status {:?}, got {described}",
        expected.status
    );
    // The envelope's own view has to hold on its own merits, not only by agreeing with the
    // listed entry: a pair of reads that both omitted `State` would agree about nothing.
    assert_eq!(
        described["Instance"]["State"]["Name"].as_str(),
        Some(expected.status),
        "case {case:?}: expected the envelope's State.Name {:?}, got {described}",
        expected.status
    );
    assert_eq!(
        described["Instance"]["State"], entry["State"],
        "case {case:?}: the listed entry and the described envelope report one State, got \
         {} beside {}",
        described["Instance"]["State"], entry["State"]
    );
}

/// Make the step's call over the wire and answer what the record then holds.
async fn apply_step(
    case: &str,
    step: Step,
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    instance_arn: &str,
    expected: &Expected,
) -> Expected {
    match step {
        // The step makes the enumeration it names, at the page size it names, and the rule
        // holds on every entry it serves; the assertion that follows every step then reads
        // the same enumeration again, so a view that survives one read but not the next
        // fails too. An enumeration writes nothing, so the expectation is unchanged.
        Step::Enumerate(max_results) => {
            for entry in listed(case, router, REGION, max_results).await {
                assert_state_mirrors_status(case, &entry);
            }
            expected.clone()
        }
        Step::Rename(name) => {
            served(
                case,
                router,
                "UpdateInstance",
                &json!({"InstanceArn": instance_arn, "Name": name}),
            )
            .await;
            Expected {
                status: expected.status,
                name: name.to_string(),
            }
        }
        Step::Tag(key) => {
            served(
                case,
                router,
                "TagResource",
                &json!({
                    "InstanceArn": instance_arn,
                    "ResourceArn": instance_arn,
                    "Tags": [{"Key": key, "Value": "identity"}],
                }),
            )
            .await;
            expected.clone()
        }
        Step::StartDeletion => {
            served(
                case,
                router,
                "DeleteInstance",
                &json!({"InstanceArn": instance_arn}),
            )
            .await;
            // The removal the clock owes is held open, so the record is read in the status
            // the delete left it in rather than after it has gone.
            hold_deletion(engine, SystemTime::now() + Duration::from_secs(3_600));
            Expected {
                status: STATUS_DELETE_IN_PROGRESS,
                name: expected.name.clone(),
            }
        }
    }
}

/// THE RULE over the scripts: after every step of every sequence, both reads of the
/// instance serve a `State` structure whose `Name` is the status the record then holds.
#[tokio::test]
async fn every_step_of_a_call_sequence_serves_the_instance_state_view() {
    for Script { label, steps } in SCRIPTS {
        let (router, engine) = empty_account_router();
        let instance_arn = create(label, &router).await;
        // The activation the create owes is made due, so the record is ACTIVE and the
        // writes the scripts make are available on it; the transition itself is applied by
        // the service's own clock on the next request.
        hold_activation(&engine, SystemTime::now() - Duration::from_secs(1));

        let mut expected = Expected {
            status: STATUS_ACTIVE,
            name: CREATED_NAME.to_string(),
        };
        // The view BEFORE the script runs, so each row starts from a projection the rule
        // already holds for.
        let case = format!("{label} / step 0 (the created instance)");
        assert_views(&case, &router, &instance_arn, &expected, None).await;

        for (index, step) in steps.iter().enumerate() {
            let case = format!("{label} / step {} ({step:?})", index + 1);
            expected = apply_step(&case, *step, &router, &engine, &instance_arn, &expected).await;
            let max_results = match step {
                Step::Enumerate(max_results) => *max_results,
                _ => None,
            };
            assert_views(&case, &router, &instance_arn, &expected, max_results).await;
        }
    }
}
