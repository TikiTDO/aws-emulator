// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for two refusals whose WHOLE sentence is fixed text: the shard
//! configuration of a replication group whose keyspace is not partitioned, and a serverless
//! cache whose name the account already holds.
//!
//! The rule under test: a refusal that is about the RESOURCE CLASS rather than about a value
//! the request carried states the rule alone — it names neither the identifier the request
//! sent nor the resource it collided with — so a client branching on the message reads the
//! same sentence whatever it asked for.
//!
//! * `ModifyReplicationGroupShardConfiguration` configures "the Valkey or Redis OSS (cluster
//!   mode ENABLED) cluster (replication group) on which the shards are to be configured": a
//!   group that is not one has a single keyspace in a single shard, so there is no shard
//!   configuration of it to change;
//! * `CreateServerlessCache` refuses a name the account already holds with
//!   `ServerlessCacheAlreadyExistsFault`.
//!
//! The tests are tables: the rows span the shard count asked for and the direction of the
//! change, and the casing and length of the name a second create sends, so the sentence is
//! pinned as fixed text rather than as one recorded string.
//!
//! Every assertion goes over the wire through the same router the production emulator
//! mounts, with the shared `harness` request builder and XML reader. The expected sentences
//! are written out here on purpose: a test that built them from the same constant the
//! handler reads would pass whatever that constant said.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{params, poll_until, Emulator, Reply, REGION};

/// How ElastiCache refuses a shard-configuration change on a group whose keyspace is not
/// partitioned.
const SHARDS_NEED_CLUSTER_MODE: &str =
    "Operation is only applicable for cluster mode enabled replication groups.";

/// How ElastiCache refuses a serverless cache whose name the account already holds: the
/// class alone, with no trailing stop.
const SERVERLESS_CACHE_ALREADY_EXISTS: &str = "Serverless Cache already exists";

/// Create one replication group with the pairs a row sends and let the clock settle it.
async fn create_settled_group(emulator: &Emulator, case: &str, id: &str, extra: &[(&str, &str)]) {
    let mut pairs = params(&[
        ("ReplicationGroupId", id),
        ("ReplicationGroupDescription", "a group to reshard"),
        ("Engine", state::DEFAULT_ENGINE),
        ("CacheNodeType", "cache.m7g.large"),
    ]);
    pairs.extend(params(extra));
    let created = emulator.call("CreateReplicationGroup", &pairs).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: the create must answer 200, got {} {}",
        created.status, created.body
    );
    // The create leaves the group `creating`, and only a settled group takes a reshard, so
    // the suite waits for that condition rather than for a fixed time.
    poll_until(
        emulator,
        REGION,
        case,
        &format!("the replication group {id} settling at available"),
        "DescribeReplicationGroups",
        &params(&[("ReplicationGroupId", id)]),
        |reply| reply.text("Status") == Some(state::STATUS_AVAILABLE),
    )
    .await;
}

/// `ModifyReplicationGroupShardConfiguration` of `id` to `shards` shards.
async fn reshard(emulator: &Emulator, id: &str, shards: &str) -> Reply {
    emulator
        .call(
            "ModifyReplicationGroupShardConfiguration",
            &params(&[
                ("ReplicationGroupId", id),
                ("NodeGroupCount", shards),
                ("ApplyImmediately", "true"),
            ]),
        )
        .await
}

/// `CreateServerlessCache` for one name.
async fn create_serverless_cache(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call(
            "CreateServerlessCache",
            &params(&[("ServerlessCacheName", name), ("Engine", "valkey")]),
        )
        .await
}

/// A reshard of a group whose keyspace is not partitioned is refused with the rule, whatever
/// count it asked for.
#[tokio::test]
async fn a_reshard_of_an_unpartitioned_group_states_the_cluster_mode_rule() {
    let emulator = Emulator::new();
    // Two groups that are not cluster mode enabled: one created with no partitioning member
    // at all, and one that asked for the single shard a group has by default.
    create_settled_group(&emulator, "the default group", "unsharded-default", &[]).await;
    create_settled_group(
        &emulator,
        "the group that asked for one shard",
        "unsharded-one",
        &[("NumNodeGroups", "1")],
    )
    .await;
    // And one that IS cluster mode enabled, so the rule is shown to bite only the groups it
    // is about.
    create_settled_group(
        &emulator,
        "the partitioned group",
        "sharded-group",
        &[("NumNodeGroups", "2"), ("ReplicasPerNodeGroup", "1")],
    )
    .await;

    // The axis: the group the request names and the count it asks for — a scale-out, a
    // bigger scale-out, and the published maximum — none of which a group without a
    // partitioned keyspace can take.
    for (case, id, shards) in [
        ("one shard to two", "unsharded-default", "2"),
        ("one shard to five", "unsharded-default", "5"),
        (
            "one shard to the published maximum",
            "unsharded-one",
            "90",
        ),
    ] {
        let refused = reshard(&emulator, id, shards).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (400, "InvalidParameterValue"),
            "case {case:?}: expected 400 InvalidParameterValue, got {} {}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.error_message(),
            SHARDS_NEED_CLUSTER_MODE,
            "case {case:?}: expected the cluster-mode rule, got {:?}",
            refused.error_message()
        );
    }

    // The same call on a group whose keyspace IS partitioned is served.
    let accepted = reshard(&emulator, "sharded-group", "3").await;
    assert_eq!(
        accepted.status, 200,
        "a partitioned group must take a reshard, got {} {}",
        accepted.status, accepted.body
    );
}

/// A serverless cache whose name the account already holds is refused with the class's own
/// sentence, whatever name it was.
#[tokio::test]
async fn a_second_serverless_cache_of_one_name_states_the_class_alone() {
    let emulator = Emulator::new();

    // The axis: the name a first create took, and how the second create spells it — a
    // serverless cache name "is stored as a lowercase string", so a name in another case is
    // the same cache.
    let rows: &[(&str, &str, &str)] = &[
        ("the same name again", "taken-cache", "taken-cache"),
        ("the same name in upper case", "cased-cache", "CASED-CACHE"),
        (
            "a long name the account already holds",
            "a-serverless-cache-name-of-some-length",
            "A-Serverless-Cache-Name-Of-Some-Length",
        ),
    ];

    for (case, first, second) in rows {
        let created = create_serverless_cache(&emulator, first).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the first create must answer 200, got {} {}",
            created.status, created.body
        );

        let refused = create_serverless_cache(&emulator, second).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (400, "ServerlessCacheAlreadyExistsFault"),
            "case {case:?}: expected 400 ServerlessCacheAlreadyExistsFault, got {} {}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.error_message(),
            SERVERLESS_CACHE_ALREADY_EXISTS,
            "case {case:?}: expected the class's own sentence, got {:?}",
            refused.error_message()
        );
        // The cache the first create made is untouched: the refusal replaced nothing.
        let described = emulator
            .call(
                "DescribeServerlessCaches",
                &params(&[("ServerlessCacheName", first)]),
            )
            .await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: the first cache must still be there, got {} {}",
            described.status, described.body
        );
    }
}
