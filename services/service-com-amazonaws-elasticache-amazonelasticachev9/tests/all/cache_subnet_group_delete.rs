// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteCacheSubnetGroup` and what the store looks like
//! afterwards, read back through `DescribeCacheSubnetGroups`.
//!
//! The rules under test:
//!
//! * a delete REMOVES the group from the account, so the describe that follows
//!   answers the modelled `CacheSubnetGroupNotFoundFault` (400), a second delete
//!   answers the same, the name is free for a create again, and no other group is
//!   touched;
//! * "The requested cache subnet group name does not refer to an existing cache
//!   subnet group" — a name this account holds no group under is that modelled
//!   refusal, worded with the name the REQUEST spelled;
//! * a name no cache subnet group could carry ("Must contain no more than 255
//!   alphanumeric characters or hyphens") is refused as the bad parameter it is,
//!   rather than reported as a group that does not exist;
//! * "You cannot delete … one that is associated with any clusters" — the
//!   modelled `CacheSubnetGroupInUse` (400) while a cache cluster or a
//!   replication group is placed in the group, which is still there afterwards,
//!   and the delete succeeds once that resource is gone;
//! * the name is resolved as the API documents it (`CacheSubnetGroupName` "is
//!   stored as a lowercase string"), so the casing a delete spells it in does not
//!   matter.
//!
//! Every assertion goes over the wire through the same router the production
//! emulator mounts, reusing the harness in [`crate::cache_cluster_lifecycle`];
//! nothing reaches into the service's state struct, so a response that is not
//! computed from persisted state cannot pass.
//!
//! The tests are tables: each row is one input along the axis the rule
//! generalises over (the name's case, the number of subnets the group holds, the
//! tag count, the kind of resource holding the group, the way a name can be
//! malformed), so no single recorded request is what makes them pass.

use crate::cache_cluster_lifecycle::{
    account_subnet_ids, create_available_cluster, subnet_group_params,
};
use crate::harness::{params, Emulator, Reply, PAST_TRANSITION};

/// Create one cache subnet group holding `subnets`, with whatever tags `tags`
/// names.
async fn create_group(
    emulator: &Emulator,
    name: &str,
    description: &str,
    subnets: &[String],
    tags: &[(&str, &str)],
) -> Reply {
    let mut pairs = subnet_group_params(name, description, subnets);
    for (index, (key, value)) in tags.iter().enumerate() {
        pairs.push((format!("Tags.Tag.{}.Key", index + 1), (*key).to_string()));
        pairs.push((
            format!("Tags.Tag.{}.Value", index + 1),
            (*value).to_string(),
        ));
    }
    emulator.call("CreateCacheSubnetGroup", &pairs).await
}

/// Delete the group `name` names.
async fn delete_group(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call(
            "DeleteCacheSubnetGroup",
            &params(&[("CacheSubnetGroupName", name)]),
        )
        .await
}

/// Describe the group `name` names.
async fn describe_group(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call(
            "DescribeCacheSubnetGroups",
            &params(&[("CacheSubnetGroupName", name)]),
        )
        .await
}

/// Every `CacheSubnetGroupName` a describe of the whole account reports.
async fn all_group_names(emulator: &Emulator) -> Vec<String> {
    let reply = emulator
        .call(
            "DescribeCacheSubnetGroups",
            &params(&[("MaxRecords", "100")]),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "a describe of every group must answer 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .all("CacheSubnetGroups/CacheSubnetGroup/CacheSubnetGroupName")
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// The sentence this operation words a cache subnet group it cannot find with,
/// built from the name's STORED identity — `CacheSubnetGroupName` "is stored as a
/// lowercase string", so the service names a group under that one identity
/// whatever case the request spelled. The delete's own template, which is not the
/// describe's (`Cache subnet group <name> not found.`).
fn not_found_message(name: &str) -> String {
    format!(
        "Cache Subnet Group {} does not exist.",
        name.to_ascii_lowercase()
    )
}

/// The sentence this operation refuses a group a cluster is placed in with, built
/// from the name's stored identity; see [`not_found_message`].
fn in_use_message(name: &str) -> String {
    format!(
        "Cache subnet group {} is currently in use.",
        name.to_ascii_lowercase()
    )
}

/// Assert that `name` names no group any more: the describe and a second delete
/// both answer the modelled not-found, with the message built from the request.
async fn assert_absent(emulator: &Emulator, case: &str, name: &str) {
    let described = describe_group(emulator, name).await;
    assert_eq!(
        described.status, 400,
        "case {case:?}: a describe of the deleted group must answer 400, got {} {}",
        described.status, described.body
    );
    assert_eq!(
        described.error_code(),
        "CacheSubnetGroupNotFoundFault",
        "case {case:?}: expected the modelled not-found code, got {}",
        described.body
    );
    let again = delete_group(emulator, name).await;
    assert_eq!(
        again.status, 400,
        "case {case:?}: a second delete must answer 400, got {} {}",
        again.status, again.body
    );
    assert_eq!(
        again.error_code(),
        "CacheSubnetGroupNotFoundFault",
        "case {case:?}: expected the modelled not-found code on the second delete, got {}",
        again.body
    );
    assert_eq!(
        again.error_message(),
        not_found_message(name),
        "case {case:?}: expected the not-found sentence for {name:?}, got {}",
        again.body
    );
}

/// One delete row: the case, the name the group is created with, the name the
/// delete spells it with, how many of the account's subnets it holds, and the
/// tags the create carries.
type DeleteCase = (
    &'static str,
    &'static str,
    &'static str,
    usize,
    &'static [(&'static str, &'static str)],
);

/// The rows span what the rule generalises over: the case the name is created
/// and deleted in, the number of subnets the group holds (one, two, every subnet
/// the account has), and the tag count.
const DELETE_CASES: &[DeleteCase] = &[
    (
        "a lowercase name holding one subnet, deleted as spelled",
        "plain-subnets",
        "plain-subnets",
        1,
        &[],
    ),
    (
        "a mixed-case name holding two subnets, deleted in upper case",
        "Mixed-Case-Subnets",
        "MIXED-CASE-SUBNETS",
        2,
        &[],
    ),
    (
        "an upper-case name holding every subnet, deleted in lower case",
        "UPPER-SUBNETS",
        "upper-subnets",
        3,
        &[("team", "cache")],
    ),
    (
        "a tagged group",
        "tagged-subnets",
        "tagged-subnets",
        2,
        &[("team", "cache"), ("env", "test"), ("owner", "nobody")],
    ),
];

/// A delete removes the group: the describe that follows answers the modelled
/// not-found, a second delete answers the same, the name is free to create again,
/// and the groups beside it are untouched.
#[tokio::test]
async fn a_delete_removes_the_subnet_group_from_the_account() {
    let emulator = Emulator::new();
    let subnets = account_subnet_ids(&emulator).await;
    let before = all_group_names(&emulator).await;

    for (case, created_as, deleted_as, subnet_count, tags) in DELETE_CASES {
        assert!(
            *subnet_count <= subnets.len(),
            "case {case:?}: the account has {} subnets, so {subnet_count} cannot be named",
            subnets.len()
        );
        let held = &subnets[..*subnet_count];
        let created = create_group(&emulator, created_as, "a group to delete", held, tags).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the create must succeed, got {} {}",
            created.status, created.body
        );
        assert_eq!(
            created
                .text("CacheSubnetGroup/CacheSubnetGroupName")
                .unwrap_or_default(),
            created_as.to_ascii_lowercase(),
            "case {case:?}: the create must persist the lowercased name, got {}",
            created.body
        );

        let described = describe_group(&emulator, deleted_as).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: the group must be readable before the delete, got {} {}",
            described.status, described.body
        );
        assert_eq!(
            described.all("Subnets/Subnet/SubnetIdentifier").len(),
            *subnet_count,
            "case {case:?}: the stored group must hold {subnet_count} subnets, got {}",
            described.body
        );

        let deleted = delete_group(&emulator, deleted_as).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: the delete must answer 200, got {} {}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.error_code(),
            "",
            "case {case:?}: the delete must carry no error envelope, got {}",
            deleted.body
        );

        assert_absent(&emulator, case, deleted_as).await;

        // The name is free again: the create that would have collided before now
        // succeeds, which is what a client that deleted a group to recreate it
        // depends on — and the recreated group holds the subnets THIS create
        // named, not the ones the deleted record held.
        let recreated =
            create_group(&emulator, created_as, "the same name again", &subnets[..1], &[]).await;
        assert_eq!(
            recreated.status, 200,
            "case {case:?}: the name must be free to create again, got {} {}",
            recreated.status, recreated.body
        );
        let reread = describe_group(&emulator, deleted_as).await;
        assert_eq!(
            reread.all("Subnets/Subnet/SubnetIdentifier"),
            vec![subnets[0].as_str()],
            "case {case:?}: the recreated group must hold the subnets its own create named, got {}",
            reread.body
        );
        let cleaned = delete_group(&emulator, deleted_as).await;
        assert_eq!(
            cleaned.status, 200,
            "case {case:?}: the recreated group must delete too, got {} {}",
            cleaned.status, cleaned.body
        );
        assert_absent(&emulator, case, deleted_as).await;
    }

    // Nothing else left with them: the account holds exactly the groups it
    // started with.
    let after = all_group_names(&emulator).await;
    assert_eq!(
        after, before,
        "the deletes must leave every other group in place, expected {before:?}, got {after:?}"
    );
}

/// Deleting two of several groups removes exactly those two: a delete is not a
/// clear of the map.
#[tokio::test]
async fn a_delete_touches_only_the_subnet_group_it_names() {
    let emulator = Emulator::new();
    let subnets = account_subnet_ids(&emulator).await;

    for name in ["keep-one", "drop-one", "keep-two", "drop-two"] {
        let created = create_group(&emulator, name, "one of four", &subnets[..2], &[]).await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: the create must succeed, got {} {}",
            created.status, created.body
        );
    }

    for name in ["drop-one", "drop-two"] {
        let deleted = delete_group(&emulator, name).await;
        assert_eq!(
            deleted.status, 200,
            "case {name:?}: the delete must answer 200, got {} {}",
            deleted.status, deleted.body
        );
    }

    let mut after = all_group_names(&emulator).await;
    after.sort();
    let expected = vec!["keep-one".to_string(), "keep-two".to_string()];
    assert_eq!(
        after, expected,
        "expected only the named groups removed, expected {expected:?}, got {after:?}"
    );

    for name in ["keep-one", "keep-two"] {
        let described = describe_group(&emulator, name).await;
        assert_eq!(
            described.status, 200,
            "case {name:?}: the kept group must still be readable, got {} {}",
            described.status, described.body
        );
        assert_eq!(
            described.all("Subnets/Subnet/SubnetIdentifier").len(),
            2,
            "case {name:?}: the kept group must still hold its subnets, got {}",
            described.body
        );
    }
    for name in ["drop-one", "drop-two"] {
        assert_absent(&emulator, name, name).await;
    }
}

/// A name this account holds no group under is the modelled not-found, worded
/// with the name the request spelled. The rows span a name nothing ever created,
/// a name only another group's prefix matches, and the published length bound
/// (255) — a name AT the bound is well formed, so it is a not-found too.
#[tokio::test]
async fn a_subnet_group_this_account_does_not_hold_is_the_modelled_not_found() {
    let emulator = Emulator::new();
    let subnets = account_subnet_ids(&emulator).await;
    let created = create_group(&emulator, "held-subnets", "a group that exists", &subnets, &[]).await;
    assert_eq!(
        created.status, 200,
        "the group that exists must be created, got {} {}",
        created.status, created.body
    );

    // Source of the 255 bound: `CacheSubnetGroupName`'s own constraint — "Must
    // contain no more than 255 alphanumeric characters or hyphens".
    let longest = "g".repeat(255);
    let rows: [(&str, &str); 5] = [
        ("a name nothing ever created", "never-created-subnets"),
        (
            "the same name in another case",
            "Never-Created-Subnets",
        ),
        ("a prefix of a group that exists", "held"),
        ("a single character", "g"),
        ("a name at the published length bound", longest.as_str()),
    ];

    for (case, name) in rows {
        let refused = delete_group(&emulator, name).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "CacheSubnetGroupNotFoundFault",
            "case {case:?}: expected the modelled not-found code, got {}",
            refused.body
        );
        assert_eq!(
            refused.error_message(),
            not_found_message(name),
            "case {case:?}: expected the not-found sentence for {name:?}, got {}",
            refused.body
        );
    }

    // And the group that does exist is untouched by the refusals above.
    let described = describe_group(&emulator, "held-subnets").await;
    assert_eq!(
        described.status, 200,
        "the group that exists must still be readable, got {} {}",
        described.status, described.body
    );
}

/// A name no cache subnet group could carry is refused as the bad parameter it
/// is, before it is resolved against anything: the modelled `InvalidParameterValue`,
/// not a report that the group does not exist. The rows span each way the
/// documented form can be broken, with the name one character PAST the published
/// bound beside the one AT it (which the test above pins as a not-found).
#[tokio::test]
async fn a_name_no_subnet_group_could_carry_is_refused_as_a_bad_parameter() {
    let emulator = Emulator::new();
    let too_long = "g".repeat(256);
    let rows: [(&str, &str); 4] = [
        ("one character past the published bound", too_long.as_str()),
        ("an underscore", "under_score-subnets"),
        ("a dot", "subnets.one"),
        ("a space", "two words"),
    ];

    for (case, name) in rows {
        let refused = delete_group(&emulator, name).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidParameterValue",
            "case {case:?}: expected the parameter refusal rather than a not-found, got {}",
            refused.body
        );
        assert!(
            refused.error_message().contains("CacheSubnetGroupName"),
            "case {case:?}: expected the refusal to name the member, got {}",
            refused.body
        );
    }
}

/// A group a cache cluster or a replication group is placed in cannot be
/// deleted, and can once that resource is gone.
#[tokio::test]
async fn a_subnet_group_in_use_cannot_be_deleted() {
    let emulator = Emulator::new();
    let subnets = account_subnet_ids(&emulator).await;

    for name in ["cluster-subnets", "replication-group-subnets"] {
        let created = create_group(&emulator, name, "a group in use", &subnets[..2], &[]).await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: the create must succeed, got {} {}",
            created.status, created.body
        );
    }

    create_available_cluster(
        &emulator,
        "in-use-cluster",
        &[
            ("Engine", "redis"),
            ("CacheNodeType", "cache.t3.micro"),
            ("NumCacheNodes", "1"),
            ("CacheSubnetGroupName", "cluster-subnets"),
        ],
    )
    .await;
    let group = emulator
        .call(
            "CreateReplicationGroup",
            &params(&[
                ("ReplicationGroupId", "in-use-rg"),
                (
                    "ReplicationGroupDescription",
                    "a group holding a subnet group",
                ),
                ("CacheNodeType", "cache.t3.micro"),
                ("CacheSubnetGroupName", "replication-group-subnets"),
            ]),
        )
        .await;
    assert_eq!(
        group.status, 200,
        "the replication group must be created, got {} {}",
        group.status, group.body
    );
    std::thread::sleep(PAST_TRANSITION);

    // Each row: the case, the subnet group a resource holds, the action that
    // releases it, and that action's form pairs.
    type InUseCase = (
        &'static str,
        &'static str,
        &'static str,
        &'static [(&'static str, &'static str)],
    );
    let rows: [InUseCase; 2] = [
        (
            "a cache cluster is placed in it",
            "cluster-subnets",
            "DeleteCacheCluster",
            &[("CacheClusterId", "in-use-cluster")],
        ),
        (
            "a replication group is placed in it",
            "replication-group-subnets",
            "DeleteReplicationGroup",
            &[("ReplicationGroupId", "in-use-rg")],
        ),
    ];

    for (case, name, release_action, release_params) in rows {
        let refused = delete_group(&emulator, name).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 while the group is in use, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "CacheSubnetGroupInUse",
            "case {case:?}: expected the modelled in-use refusal, got {}",
            refused.body
        );
        assert_eq!(
            refused.error_message(),
            in_use_message(name),
            "case {case:?}: expected the in-use sentence for {name:?}, got {}",
            refused.body
        );
        // The group is resolved case-insensitively, and the refusal echoes the
        // name the REQUEST spelled rather than the stored one.
        let shouted = name.to_ascii_uppercase();
        let refused_again = delete_group(&emulator, &shouted).await;
        assert_eq!(
            refused_again.error_code(),
            "CacheSubnetGroupInUse",
            "case {case:?}: the same group named in another case must be refused too, got {}",
            refused_again.body
        );
        assert_eq!(
            refused_again.error_message(),
            in_use_message(&shouted),
            "case {case:?}: expected the in-use sentence for {shouted:?}, got {}",
            refused_again.body
        );
        let described = describe_group(&emulator, name).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: the refused group must still be there, got {} {}",
            described.status, described.body
        );

        // Release the group by removing the resource placed in it. A release path
        // that is not implemented yet (its own stub issue owns it) answers 501,
        // and the row then stops at the refusal above rather than claiming a
        // release nothing did.
        let released = emulator.call(release_action, &params(release_params)).await;
        if released.status != 200 {
            assert_eq!(
                released.status, 501,
                "case {case:?}: {release_action} must either succeed or answer the 501 channel, \
                 got {} {}",
                released.status, released.body
            );
            continue;
        }
        let deleted = delete_group(&emulator, name).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: the delete must succeed once the group is free, got {} {}",
            deleted.status, deleted.body
        );
        assert_absent(&emulator, case, name).await;
    }
}

/// The delete is recorded as an event of the group, beside the create — the log a
/// `DescribeEvents` reader sees, which is the other place a state change shows up.
#[tokio::test]
async fn a_subnet_group_delete_is_recorded_as_an_event() {
    let emulator = Emulator::new();
    let subnets = account_subnet_ids(&emulator).await;

    let created = create_group(&emulator, "Logged-Subnets", "a logged group", &subnets[..1], &[]).await;
    assert_eq!(
        created.status, 200,
        "the create must succeed, got {} {}",
        created.status, created.body
    );
    let deleted = delete_group(&emulator, "logged-subnets").await;
    assert_eq!(
        deleted.status, 200,
        "the delete must succeed, got {} {}",
        deleted.status, deleted.body
    );

    let events = emulator
        .call(
            "DescribeEvents",
            &params(&[
                ("SourceType", "cache-subnet-group"),
                // The event is recorded under the stored (lowercased) name, the
                // identity the create recorded it under too.
                ("SourceIdentifier", "logged-subnets"),
            ]),
        )
        .await;
    assert_eq!(
        events.status, 200,
        "the event read must answer 200, got {} {}",
        events.status, events.body
    );
    let messages = events.all("Event/Message");
    assert_eq!(
        messages.len(),
        2,
        "expected the create and the delete to be logged, got {messages:?} from {}",
        events.body
    );
    assert!(
        messages
            .iter()
            .any(|message| message.to_ascii_lowercase().contains("deleted")),
        "expected an event recording the delete, got {messages:?} from {}",
        events.body
    );
}
