// SPDX-License-Identifier: CC0-1.0

//! The three MEMBERSHIP rules this service enforces on its own resources: which user name
//! is unique and where, which replication-group shapes a create accepts, and which member
//! cluster a delete may take.
//!
//! * A user's identity is its `UserId`; its user NAME is what the engine's ACL calls it
//!   inside a user group, so the name is unique within a GROUP and not across the account
//!   — which is what lets a caller create its own user named `default`, the member every
//!   user group needs.
//! * Automatic failover promotes a replica, so a cluster mode DISABLED group needs one;
//!   for a cluster mode ENABLED group it "must be enabled" whatever each shard holds, so
//!   shards without replicas are accepted and disabling it is what is refused.
//! * A cluster that is the PRIMARY of a replication group cannot be deleted through
//!   `DeleteCacheCluster`: the group owns it, and the whole group is what a caller deletes
//!   instead.
//!
//! Each test is a table over what its rule generalises over, driven over the wire through
//! the same router the production emulator mounts.

use crate::harness::{params, poll_until, Emulator, REGION};

/// Create one user with `UserId` and `UserName`, authenticating with no password.
async fn create_user(emulator: &Emulator, id: &str, name: &str, access: &str) -> crate::harness::Reply {
    emulator
        .call(
            "CreateUser",
            &params(&[
                ("UserId", id),
                ("UserName", name),
                ("Engine", "redis"),
                ("AccessString", access),
                ("NoPasswordRequired", "true"),
            ]),
        )
        .await
}

#[tokio::test]
async fn a_user_name_is_unique_within_a_group_and_not_across_the_account() {
    let emulator = Emulator::new();
    // The axis: WHICH name a second user carries — a name another user of the account
    // already has, the name the built-in user has, and a name nobody has. None of the
    // three is a collision: the id is the identity.
    let cases: [(&str, &str, &str); 3] = [
        ("first", "shared-name", "on ~* +@all"),
        ("second", "shared-name", "on ~* +@all"),
        ("own-default", "default", "off -@all"),
    ];
    for (id, name, access) in cases {
        let created = create_user(&emulator, id, name, access).await;
        assert_eq!(
            created.status, 200,
            "case {id} ({name}): a fresh UserId must be accepted whatever name it carries, got \
             {} {}",
            created.status, created.body
        );
        assert_eq!(
            created.text("UserName"),
            Some(name),
            "case {id}: the user must report the name it was created with, got {}",
            created.body
        );
    }
    // The id IS the identity, so the same id again is refused.
    let repeated = create_user(&emulator, "first", "another-name", "on ~* +@all").await;
    assert_eq!(
        (repeated.status, repeated.error_code()),
        (400, "UserAlreadyExists"),
        "the same UserId again must be refused UserAlreadyExists, got {}",
        repeated.body
    );

    // A GROUP is what a user name is unique within: two members sharing one is the
    // collision `DuplicateUserNameFault` answers.
    let collision = emulator
        .call(
            "CreateUserGroup",
            &params(&[
                ("UserGroupId", "colliding"),
                ("Engine", "redis"),
                ("UserIds.member.1", "own-default"),
                ("UserIds.member.2", "first"),
                ("UserIds.member.3", "second"),
            ]),
        )
        .await;
    assert_eq!(
        (collision.status, collision.error_code()),
        (400, "DuplicateUserName"),
        "two members sharing a user name must be refused DuplicateUserName, got {}",
        collision.body
    );

    // The group a caller builds the AWS way: its `default`-named member is the caller's
    // OWN user, not the built-in one.
    let accepted = emulator
        .call(
            "CreateUserGroup",
            &params(&[
                ("UserGroupId", "own-default-group"),
                ("Engine", "redis"),
                ("UserIds.member.1", "own-default"),
                ("UserIds.member.2", "first"),
            ]),
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "a group whose `default`-named member is the caller's own user must be accepted, got {} {}",
        accepted.status, accepted.body
    );
    // And a group with no `default`-named member at all is refused, whichever users it
    // does hold.
    let without = emulator
        .call(
            "CreateUserGroup",
            &params(&[
                ("UserGroupId", "no-default-group"),
                ("Engine", "redis"),
                ("UserIds.member.1", "first"),
            ]),
        )
        .await;
    assert_eq!(
        (without.status, without.error_code()),
        (400, "DefaultUserRequired"),
        "a group with no member named default must be refused DefaultUserRequired, got {}",
        without.body
    );
    // Taking the `default`-named member away is the same rule on the other writer — once
    // the clock has made the group active, which is the only status it takes a change in.
    poll_until(
        &emulator,
        REGION,
        "own-default-group",
        "the user group reaching active",
        "DescribeUserGroups",
        &params(&[("UserGroupId", "own-default-group")]),
        |reply| reply.text("member/Status") == Some("active"),
    )
    .await;
    let removal = emulator
        .call(
            "ModifyUserGroup",
            &params(&[
                ("UserGroupId", "own-default-group"),
                ("UserIdsToRemove.member.1", "own-default"),
            ]),
        )
        .await;
    assert_eq!(
        (removal.status, removal.error_code()),
        (400, "DefaultUserRequired"),
        "a change that would leave the group with no member named default must be refused, got {}",
        removal.body
    );
}

#[tokio::test]
async fn automatic_failover_is_mandatory_for_cluster_mode_and_needs_a_replica_without_it() {
    let emulator = Emulator::new();
    // The axis: the partitioning the request asks for, how many replicas each shard gets,
    // and what it says about automatic failover. The recorded cluster-mode-enabled create
    // with no replicas is one row: (case, the form pairs beside the identity, the awsQuery
    // code it must be refused with, the failover setting it settles with).
    type FailoverCase = (
        &'static str,
        &'static [(&'static str, &'static str)],
        Option<&'static str>,
        &'static str,
    );
    let cases: [FailoverCase; 7] = [
        (
            "cluster mode enabled with no replicas",
            &[
                ("NumNodeGroups", "2"),
                ("ReplicasPerNodeGroup", "0"),
                ("AutomaticFailoverEnabled", "true"),
            ],
            None,
            "enabled",
        ),
        (
            "cluster mode enabled with replicas",
            &[
                ("NumNodeGroups", "2"),
                ("ReplicasPerNodeGroup", "1"),
                ("AutomaticFailoverEnabled", "true"),
            ],
            None,
            "enabled",
        ),
        (
            "cluster mode enabled saying nothing about failover",
            &[("NumNodeGroups", "3"), ("ReplicasPerNodeGroup", "0")],
            None,
            "enabled",
        ),
        (
            "cluster mode enabled turning failover off",
            &[
                ("NumNodeGroups", "2"),
                ("ReplicasPerNodeGroup", "0"),
                ("AutomaticFailoverEnabled", "false"),
            ],
            Some("InvalidParameterCombination"),
            "",
        ),
        (
            "cluster mode disabled with a replica",
            &[
                ("NumCacheClusters", "2"),
                ("AutomaticFailoverEnabled", "true"),
            ],
            None,
            "enabled",
        ),
        (
            "cluster mode disabled with nothing to promote",
            &[
                ("NumCacheClusters", "1"),
                ("AutomaticFailoverEnabled", "true"),
            ],
            Some("InvalidParameterCombination"),
            "",
        ),
        (
            "cluster mode disabled saying nothing about failover",
            &[("NumCacheClusters", "2")],
            None,
            "disabled",
        ),
    ];
    for (index, (case, extra, refusal, failover)) in cases.iter().enumerate() {
        let id = format!("failover-{index}");
        let mut pairs = params(&[
            ("ReplicationGroupId", &id),
            ("ReplicationGroupDescription", "the failover rule under test"),
            ("Engine", "redis"),
            ("CacheNodeType", "cache.m5.large"),
        ]);
        pairs.extend(params(extra));
        let created = emulator.call("CreateReplicationGroup", &pairs).await;
        match refusal {
            Some(code) => {
                assert_eq!(
                    (created.status, created.error_code()),
                    (400, *code),
                    "case {case}: expected 400 {code}, got {} {}",
                    created.status,
                    created.body
                );
                continue;
            }
            None => assert_eq!(
                created.status, 200,
                "case {case}: the create must be accepted, got {} {}",
                created.status, created.body
            ),
        }
        // The shape the group settles into is the one the request asked for.
        let described = poll_until(
            &emulator,
            REGION,
            case,
            "the group reaching available",
            "DescribeReplicationGroups",
            &params(&[("ReplicationGroupId", &id)]),
            |reply| reply.text("ReplicationGroup/Status") == Some("available"),
        )
        .await;
        assert_eq!(
            described.text("ReplicationGroup/AutomaticFailover"),
            Some(*failover),
            "case {case}: expected AutomaticFailover {failover:?}, got {}",
            described.body
        );
        let requested_shards = extra
            .iter()
            .find(|(member, _)| *member == "NumNodeGroups")
            .map_or(1, |(_, value)| value.parse::<usize>().unwrap_or(1));
        assert_eq!(
            described.all("NodeGroup/NodeGroupId").len(),
            requested_shards,
            "case {case}: expected {requested_shards} node group(s), got {}",
            described.body
        );
    }
}

#[tokio::test]
async fn a_member_serving_as_primary_cannot_be_deleted_through_delete_cache_cluster() {
    let emulator = Emulator::new();
    // The axis: whether the group has automatic failover, and how many members it holds —
    // the rule is about the ROLE, so the primary is refused in every row and a non-primary
    // member of the same group is taken.
    let cases: [(&str, &[(&str, &str)]); 2] = [
        (
            "with automatic failover",
            &[("NumCacheClusters", "3"), ("AutomaticFailoverEnabled", "true")],
        ),
        ("without automatic failover", &[("NumCacheClusters", "2")]),
    ];
    for (case, extra) in cases {
        let id = format!("primary-{}", extra.len());
        let mut pairs = params(&[
            ("ReplicationGroupId", &id),
            ("ReplicationGroupDescription", "the primary rule under test"),
            ("Engine", "redis"),
        ]);
        pairs.extend(params(extra));
        let created = emulator.call("CreateReplicationGroup", &pairs).await;
        assert_eq!(
            created.status, 200,
            "case {case}: the group must be created, got {} {}",
            created.status, created.body
        );
        let described = poll_until(
            &emulator,
            REGION,
            case,
            "the group reaching available",
            "DescribeReplicationGroups",
            &params(&[("ReplicationGroupId", &id)]),
            |reply| reply.text("ReplicationGroup/Status") == Some("available"),
        )
        .await;
        // WHICH member is the primary is the group's own answer, read off its members'
        // roles rather than guessed from an id.
        let roles = described.all("NodeGroupMember/CurrentRole");
        let members = described.all("NodeGroupMember/CacheClusterId");
        let primary = members
            .iter()
            .zip(&roles)
            .find(|(_, role)| **role == "primary")
            .map(|(member, _)| (*member).to_string())
            .unwrap_or_else(|| panic!("case {case}: the group must report a primary, got {}", described.body));
        let replica = members
            .iter()
            .zip(&roles)
            .find(|(_, role)| **role != "primary")
            .map(|(member, _)| (*member).to_string())
            .unwrap_or_else(|| panic!("case {case}: the group must report a replica, got {}", described.body));

        let refused = emulator
            .call(
                "DeleteCacheCluster",
                &params(&[("CacheClusterId", &primary)]),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (400, "InvalidCacheClusterState"),
            "case {case}: deleting the primary must be refused InvalidCacheClusterState, got {}",
            refused.body
        );
        assert_eq!(
            refused.error_message(),
            format!(
                "Cache cluster {primary} is serving as primary for replication group {id} and \
                 cannot be deleted. To delete the entire replication group, use \
                 DeleteReplicationGroup."
            ),
            "case {case}: expected the sentence naming the cluster and its group, got {:?}",
            refused.error_message()
        );
        // The group is untouched: it still holds every member, and the same primary.
        let after = emulator
            .call(
                "DescribeReplicationGroups",
                &params(&[("ReplicationGroupId", &id)]),
            )
            .await;
        assert_eq!(
            after.all("NodeGroupMember/CacheClusterId"),
            members,
            "case {case}: the refused delete must leave the membership alone, got {}",
            after.body
        );
        assert_eq!(
            after
                .all("NodeGroupMember/CacheClusterId")
                .iter()
                .zip(after.all("NodeGroupMember/CurrentRole"))
                .find(|(_, role)| *role == "primary")
                .map(|(member, _)| *member),
            Some(primary.as_str()),
            "case {case}: the refused delete must leave the primary alone, got {}",
            after.body
        );
        // The positive control: the same call on a member that is NOT the primary is
        // taken, and the group loses exactly that member.
        let taken = emulator
            .call(
                "DeleteCacheCluster",
                &params(&[("CacheClusterId", &replica)]),
            )
            .await;
        assert_eq!(
            taken.status, 200,
            "case {case}: deleting a non-primary member must be accepted, got {} {}",
            taken.status, taken.body
        );
        let left = emulator
            .call(
                "DescribeReplicationGroups",
                &params(&[("ReplicationGroupId", &id)]),
            )
            .await;
        assert!(
            !left.all("NodeGroupMember/CacheClusterId").contains(&replica.as_str()),
            "case {case}: the deleted member must be gone from the group, got {}",
            left.body
        );
        assert!(
            left.all("NodeGroupMember/CacheClusterId").contains(&primary.as_str()),
            "case {case}: every other member must stay, got {}",
            left.body
        );
    }
}
