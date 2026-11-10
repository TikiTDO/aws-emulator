// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the rule that a supplied `CacheNodeType` must name a
//! node type this service publishes.
//!
//! Every assertion goes over the wire through the router the production emulator
//! mounts, reusing the harness in [`crate::cache_cluster_lifecycle`]: nothing
//! reaches into the service's state struct, so an answer that is not computed
//! from persisted state cannot pass.
//!
//! The rule under test: an operation that PERSISTS a cache node type accepts
//! exactly the `cache.<family>.<size>` names the service's published catalogue
//! holds (`smithy_query(query="doc", shape="CreateCacheClusterMessage",
//! member="CacheNodeType")`, plus the data-tiering family `DataTieringEnabled`
//! names), read case-insensitively, and refuses every other name with
//! `InvalidParameterValue` (HTTP 400) before writing anything.
//!
//! The axes the tables span: which part of the name is outside the catalogue
//! (family, size, form), which operation carries the member
//! (`CreateCacheCluster`, `ModifyCacheCluster`, `CreateReplicationGroup`,
//! `ModifyGlobalReplicationGroup`), and — for the accepted side — the whole
//! published catalogue, enumerated rather than sampled, since it is finite.

use service_com_amazonaws_elasticache_amazonelasticachev9::ops::list_allowed_node_type_modifications as catalogue;
use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::cache_cluster_lifecycle::{create_available_cluster, create_cluster, describe_cluster};
use crate::harness::{params, Emulator, PAST_TRANSITION};
use crate::modeled_error_surfaces::seed_global_datastore;

/// Names no node type the catalogue holds, one per form the refusal has to
/// cover. The size names are read off the catalogue itself: `large` is a size
/// the M5 family names but the T3 family does not, and `32xlarge` is past the
/// largest multiplier any family names.
const UNPUBLISHED: &[(&str, &str)] = &[
    // The recorded instance: a family that does not exist at all.
    ("unknown family", "cache.not.a.real.type"),
    ("another unknown family", "cache.z9.nano"),
    // A family the catalogue holds, with a size it does not name for it.
    ("unknown size", "cache.m5.enormous"),
    ("size of another family", "cache.t3.large"),
    ("size past the largest multiplier", "cache.r6g.32xlarge"),
    // The data-tiering family has no `large` size.
    ("size the data tiering family lacks", "cache.r6gd.large"),
    // A multiplier past the end of the size ladder's range.
    ("overflowing multiplier", "cache.m5.4294967295xlarge"),
    // Names that are not of the `cache.<family>.<size>` form at all.
    ("no size segment", "cache.m5"),
    ("too many segments", "cache.m5.large.extra"),
    ("no cache prefix", "m5.large"),
    ("empty", ""),
];

/// Every node type name the published catalogue holds, in catalogue order.
fn published_node_types() -> Vec<String> {
    catalogue::NODE_TYPE_FAMILIES
        .iter()
        .chain(catalogue::DATA_TIERING_NODE_TYPE_FAMILIES)
        .flat_map(|family| {
            family
                .sizes
                .iter()
                .map(move |size| format!("{}.{size}", family.prefix))
        })
        .collect()
}

#[tokio::test]
async fn every_published_node_type_is_accepted_and_read_back() {
    // The catalogue is finite and published, so this ENUMERATES it: every name
    // it holds — both generations, every family, every size — must be accepted
    // and reported back from the store.
    let names = published_node_types();
    assert!(
        names.len() > 50,
        "the published catalogue must hold the families the doc names, got {} names: {names:?}",
        names.len()
    );

    let emulator = Emulator::new();
    let mut created = Vec::new();
    for (index, node_type) in names.iter().enumerate() {
        let id = format!("node-type-{index}");
        let reply = create_cluster(&emulator, &id, &[("CacheNodeType", node_type)]).await;
        assert_eq!(
            reply.status, 200,
            "case {node_type}: a published node type must be accepted, got {} {}",
            reply.status, reply.body
        );
        created.push((id, node_type.clone()));
    }
    // One wait for the whole table: the handlers never sleep, so only the test
    // waits out the span after which the clock owes each cluster `available`.
    std::thread::sleep(PAST_TRANSITION);

    for (id, node_type) in &created {
        let described = describe_cluster(&emulator, id).await;
        assert_eq!(
            described.status, 200,
            "case {node_type}: the cluster must be read back, got {} {}",
            described.status, described.body
        );
        assert_eq!(
            described.text("CacheCluster/CacheNodeType"),
            Some(node_type.as_str()),
            "case {node_type}: the stored node type is what the create supplied, got {:?}",
            described.text("CacheCluster/CacheNodeType")
        );
    }

    // A name is read case-insensitively, and the request's own spelling is what
    // is stored: the catalogue spells every node type in lowercase, but the
    // member carries no casing rule.
    let shouted = create_cluster(
        &emulator,
        "shouted-node-type",
        &[("CacheNodeType", "CACHE.T4G.SMALL")],
    )
    .await;
    assert_eq!(
        shouted.status, 200,
        "case CACHE.T4G.SMALL: a published node type must be accepted whatever case it is \
         spelled in, got {} {}",
        shouted.status, shouted.body
    );
    assert_eq!(
        shouted.text("CacheCluster/CacheNodeType"),
        Some("CACHE.T4G.SMALL"),
        "case CACHE.T4G.SMALL: the stored node type is what the create supplied, got {:?}",
        shouted.text("CacheCluster/CacheNodeType")
    );
}

#[tokio::test]
async fn a_create_naming_a_node_type_outside_the_catalogue_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    for (index, (case, node_type)) in UNPUBLISHED.iter().enumerate() {
        let id = format!("unpublished-{index}");
        let reply = create_cluster(&emulator, &id, &[("CacheNodeType", node_type)]).await;
        assert_eq!(
            reply.status, 400,
            "case {case} ({node_type}): the create must be refused 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidParameterValue",
            "case {case} ({node_type}): expected the modelled InvalidParameterValue, got {}",
            reply.body
        );
        // WHICH of the two sentences: a value of the `cache.<family>.<size>` form that the
        // catalogue does not hold is named in the refusal, so a client can tell which member
        // to change; a value that is not of that form at all is not a node type to name, and
        // the refusal says so instead.
        let message = reply.text("Error/Message").unwrap_or_default();
        if node_type.to_ascii_lowercase().starts_with("cache.") {
            assert!(
                message.contains(node_type),
                "case {case} ({node_type}): the message must name the refused value, got \
                 {message:?}"
            );
        } else {
            assert_eq!(
                message, catalogue::NODE_TYPE_NOT_A_NODE_TYPE_MESSAGE,
                "case {case} ({node_type}): a value that is not a node type at all must be \
                 refused with the sentence that says so, got {message:?}"
            );
        }
        // Nothing was written: every cache-cluster lookup for the id answers the
        // modelled not-found rather than resolving a cluster the service refused
        // to create.
        let described = describe_cluster(&emulator, &id).await;
        assert_eq!(
            (described.status, described.error_code()),
            (404, "CacheClusterNotFound"),
            "case {case} ({node_type}): a refused create must leave no cluster behind, got {} {}",
            described.status,
            described.body
        );
    }
}

#[tokio::test]
async fn every_cache_cluster_lookup_for_a_refused_create_answers_not_found() {
    // The id the create below is refused for exists nowhere afterwards, so each
    // operation that resolves a cache cluster by id answers the modelled
    // `CacheClusterNotFound` — existence is resolved before any state gate, so
    // none of them reports a transient-looking state error for a cluster that
    // will never appear.
    let emulator = Emulator::new();
    let id = "never-created";
    let refused = create_cluster(&emulator, id, &[("CacheNodeType", "cache.not.a.real.type")]).await;
    assert_eq!(
        refused.status, 400,
        "the create must be refused 400, got {} {}",
        refused.status, refused.body
    );

    for (action, extra) in [
        ("DescribeCacheClusters", &[][..]),
        ("ModifyCacheCluster", &[("ApplyImmediately", "true")][..]),
        ("RebootCacheCluster", &[("CacheNodeIdsToReboot.member.1", "0001")][..]),
        ("ListAllowedNodeTypeModifications", &[][..]),
        ("DeleteCacheCluster", &[][..]),
    ] {
        let mut pairs = params(&[("CacheClusterId", id)]);
        pairs.extend(params(extra));
        let reply = emulator.call(action, &pairs).await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (404, "CacheClusterNotFound"),
            "case {action}: an id no cluster was created for must answer 404 CacheClusterNotFound, \
             got {} {}",
            reply.status,
            reply.body
        );
    }
}

#[tokio::test]
async fn a_modify_onto_a_node_type_outside_the_catalogue_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    create_available_cluster(&emulator, "modify-node-type", &[("CacheNodeType", "cache.t3.micro")])
        .await;

    for (case, node_type) in UNPUBLISHED {
        let reply = emulator
            .call(
                "ModifyCacheCluster",
                &params(&[
                    ("CacheClusterId", "modify-node-type"),
                    ("CacheNodeType", node_type),
                    ("ApplyImmediately", "true"),
                ]),
            )
            .await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (400, "InvalidParameterValue"),
            "case {case} ({node_type}): the modify must be refused 400 InvalidParameterValue, got \
             {} {}",
            reply.status,
            reply.body
        );
        // A refused modify leaves the stored node type where it was, and records
        // nothing as pending either.
        let described = describe_cluster(&emulator, "modify-node-type").await;
        assert_eq!(
            described.text("CacheCluster/CacheNodeType"),
            Some("cache.t3.micro"),
            "case {case} ({node_type}): a refused modify must not change the stored node type, \
             got {}",
            described.body
        );
        assert_eq!(
            described.text("PendingModifiedValues/CacheNodeType"),
            None,
            "case {case} ({node_type}): a refused modify must record nothing as pending, got {}",
            described.body
        );
    }

    // The same operation DOES move the cluster onto a published node type, so
    // the refusals above are about the catalogue and not about the member being
    // rejected outright. `cache.r6gd.2xlarge` is deliberately ungated here: the
    // rule under test is PUBLICATION (is this a node type at all), not
    // data-tiering eligibility ("only supported for replication groups using
    // the r6gd node type"), which is a separate rule this change does not add —
    // `CreateReplicationGroup` is where `DataTieringEnabled` is enforced today.
    for node_type in ["cache.m5.large", "cache.r6gd.2xlarge", "cache.m3.medium"] {
        let applied = emulator
            .call(
                "ModifyCacheCluster",
                &params(&[
                    ("CacheClusterId", "modify-node-type"),
                    ("CacheNodeType", node_type),
                    ("ApplyImmediately", "true"),
                ]),
            )
            .await;
        assert_eq!(
            applied.status, 200,
            "case {node_type}: a published node type must be accepted, got {} {}",
            applied.status, applied.body
        );
        std::thread::sleep(PAST_TRANSITION);
        let described = describe_cluster(&emulator, "modify-node-type").await;
        assert_eq!(
            described.text("CacheCluster/CacheNodeType"),
            Some(node_type),
            "case {node_type}: the applied node type must be read back, got {}",
            described.body
        );
    }
}

#[tokio::test]
async fn a_replication_group_create_follows_the_same_catalogue() {
    let emulator = Emulator::new();
    for (index, (case, node_type)) in UNPUBLISHED.iter().enumerate() {
        let group_id = format!("rg-unpublished-{index}");
        let reply = emulator
            .call(
                "CreateReplicationGroup",
                &params(&[
                    ("ReplicationGroupId", group_id.as_str()),
                    ("ReplicationGroupDescription", "an emulated replication group"),
                    ("CacheNodeType", node_type),
                ]),
            )
            .await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (400, "InvalidParameterValue"),
            "case {case} ({node_type}): the create must be refused 400 InvalidParameterValue, got \
             {} {}",
            reply.status,
            reply.body
        );
        let described = emulator
            .call(
                "DescribeReplicationGroups",
                &params(&[("ReplicationGroupId", group_id.as_str())]),
            )
            .await;
        assert_eq!(
            described.status, 404,
            "case {case} ({node_type}): a refused create must leave no group behind, got {} {}",
            described.status, described.body
        );
    }

    // The accepted side, including the data-tiering family the catalogue's own
    // list omits and `DataTieringEnabled` names: "Data tiering is only supported
    // for replication groups using the r6gd node type. This parameter must be
    // set to true when using r6gd nodes."
    for (id, node_type, extra) in [
        ("rg-published", "cache.m7g.large", &[][..]),
        (
            "rg-data-tiering",
            "cache.r6gd.2xlarge",
            &[("DataTieringEnabled", "true")][..],
        ),
    ] {
        let mut pairs = params(&[
            ("ReplicationGroupId", id),
            ("ReplicationGroupDescription", "an emulated replication group"),
            ("CacheNodeType", node_type),
        ]);
        pairs.extend(params(extra));
        let created = emulator.call("CreateReplicationGroup", &pairs).await;
        assert_eq!(
            created.status, 200,
            "case {node_type}: a published node type must be accepted, got {} {}",
            created.status, created.body
        );
        let described = emulator
            .call(
                "DescribeReplicationGroups",
                &params(&[("ReplicationGroupId", id)]),
            )
            .await;
        assert_eq!(
            described.status, 200,
            "case {node_type}: the group must be read back, got {} {}",
            described.status, described.body
        );
        assert_eq!(
            described.text("CacheNodeType"),
            Some(node_type),
            "case {node_type}: the stored node type is what the create supplied, got {:?}",
            described.text("CacheNodeType")
        );
    }
}

#[tokio::test]
async fn a_global_datastore_modify_follows_the_same_catalogue() {
    // `ModifyGlobalReplicationGroup` persists the node type it is given (the
    // datastore reports it back through DescribeGlobalReplicationGroups), so the
    // same rule holds for it. The datastore is seeded through the one store the
    // handlers read, exactly as the sibling Global-datastore suite does, and
    // everything asserted about it afterwards comes back over the wire.
    let emulator = Emulator::new();
    seed_global_datastore(&emulator, "ldgnf-node-type", 1, &[]);

    for (case, node_type) in UNPUBLISHED {
        let reply = emulator
            .call(
                "ModifyGlobalReplicationGroup",
                &params(&[
                    ("GlobalReplicationGroupId", "ldgnf-node-type"),
                    ("ApplyImmediately", "true"),
                    ("CacheNodeType", node_type),
                ]),
            )
            .await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (400, "InvalidParameterValue"),
            "case {case} ({node_type}): the modify must be refused 400 InvalidParameterValue, got \
             {} {}",
            reply.status,
            reply.body
        );
        // A refused modify leaves the datastore on the node type it was seeded
        // with, and does not start a transition either.
        let described = emulator
            .call(
                "DescribeGlobalReplicationGroups",
                &params(&[("GlobalReplicationGroupId", "ldgnf-node-type")]),
            )
            .await;
        assert_eq!(
            described.text("CacheNodeType"),
            Some("cache.m7g.large"),
            "case {case} ({node_type}): a refused modify must not change the stored node type, \
             got {}",
            described.body
        );
        assert_eq!(
            described.text("Status"),
            Some(state::STATUS_AVAILABLE),
            "case {case} ({node_type}): a refused modify must start no transition, got {}",
            described.body
        );
    }

    // The accepted side, so the refusals above are about the catalogue.
    for node_type in ["cache.r7g.large", "cache.m5.24xlarge"] {
        let modified = emulator
            .call(
                "ModifyGlobalReplicationGroup",
                &params(&[
                    ("GlobalReplicationGroupId", "ldgnf-node-type"),
                    ("ApplyImmediately", "true"),
                    ("CacheNodeType", node_type),
                ]),
            )
            .await;
        assert_eq!(
            modified.status, 200,
            "case {node_type}: a published node type must be accepted, got {} {}",
            modified.status, modified.body
        );
        std::thread::sleep(PAST_TRANSITION);
        let described = emulator
            .call(
                "DescribeGlobalReplicationGroups",
                &params(&[("GlobalReplicationGroupId", "ldgnf-node-type")]),
            )
            .await;
        assert_eq!(
            described.text("CacheNodeType"),
            Some(node_type),
            "case {node_type}: the applied node type must be read back, got {}",
            described.body
        );
    }
}

#[test]
fn the_data_tiering_family_is_the_one_the_data_tiering_rule_names() {
    // The catalogue's data-tiering family and the rule `CreateReplicationGroup`
    // enforces (`state::DATA_TIERING_NODE_TYPE`, matched against a node type's
    // name there) must name the SAME family, or a node type could pass the
    // catalogue and miss the data-tiering rule, or the other way round.
    for family in catalogue::DATA_TIERING_NODE_TYPE_FAMILIES {
        assert_eq!(
            family.prefix,
            format!("cache.{}", state::DATA_TIERING_NODE_TYPE),
            "case {}: the catalogue's data-tiering family must be the family the data-tiering \
             rule names ({})",
            family.prefix,
            state::DATA_TIERING_NODE_TYPE
        );
        for size in family.sizes {
            let name = format!("{}.{size}", family.prefix);
            assert!(
                name.contains(state::DATA_TIERING_NODE_TYPE)
                    && catalogue::is_published_node_type(&name),
                "case {name}: every data-tiering node type must be published and match the \
                 data-tiering rule"
            );
        }
    }
}
