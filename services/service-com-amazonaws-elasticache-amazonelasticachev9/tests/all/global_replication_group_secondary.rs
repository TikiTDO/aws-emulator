// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `CreateReplicationGroup` as the SECONDARY of a Global datastore.
//!
//! The rules under test:
//!
//! * `CreateReplicationGroup$GlobalReplicationGroupId` is "the name of the Global datastore"
//!   the new group JOINS: the create is accepted and the group becomes a member of that
//!   datastore in the Region the request was addressed to, so
//!   `DescribeGlobalReplicationGroups` reports it beside the primary and
//!   `DescribeReplicationGroups` reports the datastore and the group's role in it — the two
//!   records cannot disagree, because the membership is one write;
//! * a datastore gains a secondary, so it is no longer `primary-only`: it answers the
//!   transient status while ElastiCache works the association through and the clock settles
//!   it at `available`;
//! * a datastore this account does not hold is the modelled `GlobalReplicationGroupNotFound`
//!   (404), and one that cannot take a change is the modelled state fault — neither leaves a
//!   group that joined nothing behind a 200.
//!
//! The tests are tables: the rows span the Region the secondary is created in, how its own
//! keyspace is partitioned, and each way the request can be wrong, so no single recorded
//! request is what makes them pass.
//!
//! Every assertion goes over the wire through the same router the production emulator
//! mounts, with the shared `harness` request builder and XML reader.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{params, poll_until, Emulator, Reply, REGION};

/// A span far longer than any transition this service publishes, for a datastore a row holds
/// in a non-terminal status on purpose — so what the row asserts does not depend on how fast
/// the host runs.
const NEVER_SETTLES: Duration = Duration::from_secs(60 * 60);

/// The role the wire reports for each member of a Global datastore: UPPER case, whatever
/// the member's prose ("Indicates the role of the replication group, primary or secondary")
/// spells them as. Written out here rather than read off the constant the handler carries,
/// so the test pins the wire value.
const PRIMARY_ROLE: &str = "PRIMARY";
const SECONDARY_ROLE: &str = "SECONDARY";

/// The primary the datastore of every row is built around, and the datastore's own suffix.
const PRIMARY: &str = "global-primary";
const SUFFIX: &str = "secondaries";

/// Create one replication group, in `region`, with the pairs a row adds, and let the clock
/// settle it at `available`.
async fn create_settled_group(
    emulator: &Emulator,
    case: &str,
    region: &str,
    id: &str,
    extra: &[(&str, &str)],
) -> Reply {
    let mut pairs = params(&[
        ("ReplicationGroupId", id),
        ("ReplicationGroupDescription", "a global datastore member"),
        ("Engine", state::DEFAULT_ENGINE),
        ("CacheNodeType", "cache.m7g.large"),
    ]);
    pairs.extend(params(extra));
    let created = emulator
        .call_in(region, "CreateReplicationGroup", &pairs)
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: the create must answer 200, got {} {}",
        created.status, created.body
    );
    // The create leaves the group `creating`; the suite waits for the status the clock owes
    // it rather than for a fixed time.
    poll_until(
        emulator,
        region,
        case,
        &format!("the replication group {id} settling at available"),
        "DescribeReplicationGroups",
        &params(&[("ReplicationGroupId", id)]),
        |reply| reply.text("Status") == Some(state::STATUS_AVAILABLE),
    )
    .await;
    created
}

/// Mint the Global datastore every row joins, around a settled primary, and answer the id it
/// is stored under.
async fn create_datastore(emulator: &Emulator, case: &str) -> String {
    create_settled_group(emulator, case, REGION, PRIMARY, &[]).await;
    let created = emulator
        .call(
            "CreateGlobalReplicationGroup",
            &params(&[
                ("GlobalReplicationGroupIdSuffix", SUFFIX),
                ("PrimaryReplicationGroupId", PRIMARY),
            ]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: the datastore must be creatable, got {} {}",
        created.status, created.body
    );
    let id = created
        .text("GlobalReplicationGroupId")
        .unwrap_or_else(|| panic!("case {case:?}: the create must report the stored id"))
        .to_string();
    // A datastore with only its primary settles at `primary-only`, which is the status it
    // has to be in to take a member — so that is the condition the suite waits for.
    poll_until(
        emulator,
        REGION,
        case,
        &format!("the Global datastore {id} settling"),
        "DescribeGlobalReplicationGroups",
        &params(&[("GlobalReplicationGroupId", id.as_str())]),
        |reply| reply.text("Status") == Some(state::STATUS_PRIMARY_ONLY),
    )
    .await;
    id
}

/// `DescribeReplicationGroups` for one group by id, in `region`.
async fn describe_group(emulator: &Emulator, region: &str, id: &str) -> Reply {
    emulator
        .call_in(
            region,
            "DescribeReplicationGroups",
            &params(&[("ReplicationGroupId", id)]),
        )
        .await
}

/// One row of the secondary table: the case, the Region the secondary is created in, its
/// id, the form pairs it adds, and how many shards its own keyspace is partitioned over.
type SecondaryCase = (
    &'static str,
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
    usize,
);

/// A group created with a Global datastore named joins it as a secondary, and both records
/// report the membership.
#[tokio::test]
async fn a_group_created_with_a_global_datastore_joins_it_as_a_secondary() {
    // The axis: the Region the secondary is created in — a Global datastore replicates
    // across Regions, so the member's own Region is what the datastore reports for it — and
    // how the secondary's own keyspace is partitioned.
    let rows: &[SecondaryCase] = &[
        (
            "a secondary in another Region",
            "us-west-2",
            "secondary-west",
            &[],
            1,
        ),
        (
            "a sharded secondary in a third Region",
            "eu-west-1",
            "secondary-europe",
            &[("NumNodeGroups", "2"), ("ReplicasPerNodeGroup", "1")],
            2,
        ),
        (
            "a secondary in the datastore's own Region",
            REGION,
            "secondary-home",
            &[],
            1,
        ),
    ];

    for (case, region, id, extra, shards) in rows {
        let emulator = Emulator::new();
        let datastore = create_datastore(&emulator, case).await;

        let mut extra_pairs: Vec<(&str, &str)> =
            vec![("GlobalReplicationGroupId", datastore.as_str())];
        extra_pairs.extend(extra.iter().copied());
        let created = create_settled_group(&emulator, case, region, id, &extra_pairs).await;

        // THE RULE: the create is served, and the group it answers with reports the
        // datastore it joined and its role in it.
        assert_eq!(
            (
                created.text("GlobalReplicationGroupId"),
                created.text("GlobalReplicationGroupMemberRole"),
            ),
            (Some(datastore.as_str()), Some(SECONDARY_ROLE)),
            "case {case:?}: expected the create to report the datastore and the secondary \
             role, got {}",
            created.body
        );

        // The group a later read reports is the same one: the membership is persisted.
        let described = describe_group(&emulator, region, id).await;
        assert_eq!(
            (
                described.text("GlobalReplicationGroupId"),
                described.text("GlobalReplicationGroupMemberRole"),
                described.text("Status"),
            ),
            (
                Some(datastore.as_str()),
                Some(SECONDARY_ROLE),
                Some(state::STATUS_AVAILABLE),
            ),
            "case {case:?}: expected a later read to report the membership, got {}",
            described.body
        );
        assert_eq!(
            described.all("NodeGroups/NodeGroup/NodeGroupId").len(),
            *shards,
            "case {case:?}: expected the secondary's own shards, got {}",
            described.body
        );

        // And the datastore reports the member beside its primary, in the Region the
        // secondary was created in, with the datastore no longer primary-only: it settles at
        // `available` once it has a secondary, which is the condition the suite waits for.
        let datastore_read = poll_until(
            &emulator,
            REGION,
            case,
            &format!("the Global datastore {datastore} settling at available"),
            "DescribeGlobalReplicationGroups",
            &params(&[
                ("GlobalReplicationGroupId", datastore.as_str()),
                ("ShowMemberInfo", "true"),
            ]),
            |reply| reply.text("GlobalReplicationGroup/Status") == Some(state::STATUS_AVAILABLE),
        )
        .await;
        assert_eq!(
            datastore_read.status, 200,
            "case {case:?}: expected the datastore to be readable, got {} {}",
            datastore_read.status, datastore_read.body
        );
        assert_eq!(
            datastore_read.all("Members/GlobalReplicationGroupMember/ReplicationGroupId"),
            vec![PRIMARY, id],
            "case {case:?}: expected the primary and the secondary as members, got {}",
            datastore_read.body
        );
        assert_eq!(
            datastore_read.all("Members/GlobalReplicationGroupMember/Role"),
            vec![PRIMARY_ROLE, SECONDARY_ROLE],
            "case {case:?}: expected the two roles, got {}",
            datastore_read.body
        );
        assert_eq!(
            datastore_read.all("Members/GlobalReplicationGroupMember/ReplicationGroupRegion"),
            vec![REGION, *region],
            "case {case:?}: expected each member's own Region, got {}",
            datastore_read.body
        );
        assert_eq!(
            datastore_read.text("GlobalReplicationGroup/Status"),
            Some(state::STATUS_AVAILABLE),
            "case {case:?}: a datastore with a secondary settles at available, got {}",
            datastore_read.body
        );

        // The membership is one write, so the secondary can be taken out of the datastore
        // again — the pair the disassociation clears is the pair the create wrote.
        let disassociated = emulator
            .call(
                "DisassociateGlobalReplicationGroup",
                &params(&[
                    ("GlobalReplicationGroupId", datastore.as_str()),
                    ("ReplicationGroupId", id),
                    ("ReplicationGroupRegion", region),
                ]),
            )
            .await;
        assert_eq!(
            disassociated.status, 200,
            "case {case:?}: expected the secondary to be removable, got {} {}",
            disassociated.status, disassociated.body
        );
    }
}

/// A create naming a Global datastore it cannot join is refused with the datastore's own
/// modelled fault, and no group is stored.
#[tokio::test]
async fn a_create_that_cannot_join_the_named_datastore_is_refused() {
    let emulator = Emulator::new();
    let datastore = create_datastore(&emulator, "the refusal table's datastore").await;

    // Each row: the case, the datastore the request names, the status and the wire code.
    let rows: &[(&str, &str, u16, &str)] = &[
        (
            "a datastore this account does not hold",
            "ldgnf-never-created",
            404,
            "GlobalReplicationGroupNotFoundFault",
        ),
        (
            "a datastore named in another case, which no datastore is stored under",
            "LDGNF-NEVER-CREATED",
            404,
            "GlobalReplicationGroupNotFoundFault",
        ),
        (
            "a datastore busy with a change of its own",
            datastore.as_str(),
            400,
            "InvalidGlobalReplicationGroupState",
        ),
    ];

    for (index, (case, named, status, code)) in rows.iter().enumerate() {
        if *status == 400 {
            // Hold the datastore in the status a change of its own leaves it in, so the row
            // does not depend on how fast the host runs.
            emulator.with_state(|store| {
                let held = store
                    .global_replication_groups
                    .get_mut(&datastore)
                    .unwrap_or_else(|| panic!("case {case:?}: the datastore must be stored"));
                held.status = state::STATUS_MODIFYING.to_string();
                held.pending_status = Some(state::PendingTransition {
                    due: SystemTime::now() + NEVER_SETTLES,
                    status: state::STATUS_AVAILABLE.to_string(),
                });
            });
        }
        let id = format!("refused-secondary-{index}");
        let refused = emulator
            .call(
                "CreateReplicationGroup",
                &params(&[
                    ("ReplicationGroupId", id.as_str()),
                    ("ReplicationGroupDescription", "a group that joins nothing"),
                    ("Engine", state::DEFAULT_ENGINE),
                    ("CacheNodeType", "cache.m7g.large"),
                    ("GlobalReplicationGroupId", named),
                ]),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (*status, *code),
            "case {case:?}: expected {status} {code}, got {} {}",
            refused.status,
            refused.body
        );
        // Nothing was stored: a refused create leaves no group behind.
        let described = describe_group(&emulator, REGION, &id).await;
        assert_eq!(
            (described.status, described.error_code()),
            (404, "ReplicationGroupNotFoundFault"),
            "case {case:?}: a refused create must store no group, got {} {}",
            described.status,
            described.body
        );
    }
}
