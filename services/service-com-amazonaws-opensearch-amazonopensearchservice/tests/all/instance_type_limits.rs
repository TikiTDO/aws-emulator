// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeInstanceTypeLimits`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`]; this suite adds no second copy of it.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * the answer carries one entry per ROLE the named instance type can serve, over a
//!   table spanning the families that serve one role, two roles, and the warm tier;
//! * the instance-count range is the role's, and a role's range is the same whatever
//!   the instance type;
//! * an instance type with attachable volumes publishes, under the DATA role alone,
//!   one `ebs` storage type per volume sub-type THAT instance type attaches, each with
//!   the same minimum size, a maximum that grows with the instance's size up to what the
//!   instance's own EBS attachment takes, a flat cap for the magnetic sub-type, and the
//!   IOPS and throughput ranges only the provisioned sub-types carry, their ceilings
//!   bounded by the same attachment — over a table spanning the burstable class, several
//!   current-generation sizes from the smallest published to the largest, and a
//!   previous-generation family;
//! * an instance type whose data sits on built-in storage publishes that one storage
//!   type with no sub-type and no volume limits, and a warm-tier type publishes none;
//! * the additional limit is the role's, and the data nodes a master supports grow with
//!   the master's size;
//! * an `EngineVersion` that names no engine release, an `InstanceType` that names no
//!   published instance type, a pairing the release does not offer, and a `domainName`
//!   that names no domain are each refused with the modelled error;
//! * the read changes nothing a later caller sees;
//! * and the published range is the range the service ENFORCES: `CreateDomain` and
//!   `UpdateDomainConfig` refuse a volume sized outside it, at either end, with the
//!   modelled service-limit refusal, and store nothing when they do — which
//!   [`super::ebs_volume_size_limit`] holds to the class, status and wording of that
//!   refusal.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::domain_lifecycle::{
    error_code, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS, SETTLE_WAIT,
};

/// The engine versions the role and storage tables span: one release of each engine
/// that offers every family named below, so no rule rests on a single version.
const VERSIONS: [&str; 3] = ["Elasticsearch_7.10", "OpenSearch_1.0", "OpenSearch_2.19"];

/// The `VolumeType` values an `ebs` storage type can be published for, in the order the
/// service's generated `VOLUME_TYPE_VALUES` lists them. Source:
/// `smithy_query(query="shape",shape="VolumeType")` — `standard`, `gp2`, `io1`, `gp3`.
const VOLUME_SUB_TYPES: [&str; 4] = ["gp2", "gp3", "io1", "standard"];

/// The sub-types EVERY instance class with an attached volume publishes: the
/// general-purpose SSD ones, in that same order.
pub(crate) const GENERAL_PURPOSE_SUB_TYPES: &[&str] = &["gp2", "gp3"];

/// Those plus the provisioned-IOPS one, which attaches to every class whose EBS
/// attachment provisions its own performance.
pub(crate) const WITH_PROVISIONED_IOPS: &[&str] = &["gp2", "gp3", "io1"];

/// The sub-types a previous-generation family publishes: the magnetic one beside those.
pub(crate) const EVERY_SUB_TYPE: &[&str] = &["gp2", "gp3", "io1", "standard"];

/// The suite's OWN expectation of the published volume-size floor, every sub-type and
/// every instance type: restated here rather than imported from `crate::state`, so
/// changing it in the handler fails this suite instead of being asserted against
/// itself.
pub(crate) const MINIMUM_VOLUME_SIZE: i64 = 10;

/// The same for the magnetic sub-type's flat maximum.
pub(crate) const MAXIMUM_STANDARD_VOLUME_SIZE: i64 = 100;

/// One row of the storage table: an instance type, the sub-types it publishes, and the
/// three ceilings published for it — the volume size in GiB, and the IOPS and the
/// throughput a `gp3` volume on it is provisioned up to.
///
/// Shared with [`super::reference_catalogs`], which holds each row's figures to the
/// published limit blocks, so [`STORAGE_ROWS`] stays the ONE statement of the table.
pub(crate) struct StorageRow {
    /// The instance type the row is asked about.
    pub(crate) instance_type: &'static str,
    /// The sub-types its data role publishes an `ebs` storage type for, in order.
    pub(crate) sub_types: &'static [&'static str],
    /// The `MaximumVolumeSize`, in GiB, every one of those sub-types publishes (the
    /// magnetic one excepted, which carries its own flat cap).
    pub(crate) maximum_volume_size: i64,
    /// The `MaximumIops` a provisioned sub-type publishes.
    pub(crate) maximum_iops: i64,
    /// The `MaximumThroughput` the `gp3` sub-type publishes.
    pub(crate) maximum_throughput: i64,
}

/// The storage table every storage assertion below is made over, ordered so the
/// published maximum never falls along it.
///
/// The suite's OWN expectation of the published figures: a volume of 512 GiB attaches to
/// every instance type whatever its size, a `large` is four normalization units and
/// carries exactly that, so one unit carries 128 GiB and the largest published size,
/// `24xlarge` (192 units), carries 24 TiB — except that the burstable class, whose EBS
/// attachment bursts from a credit balance rather than being provisioned, publishes that
/// attachment's own ceilings (100 GiB, 11,800 IOPS, 260 MiB/s) at every one of its sizes.
/// A provisioned `gp3` volume on any other class reaches the sub-type's own ceilings
/// (16,000 IOPS, 1,000 MiB/s).
pub(crate) const STORAGE_ROWS: [StorageRow; 11] = [
    StorageRow {
        instance_type: "t3.nano.search",
        sub_types: GENERAL_PURPOSE_SUB_TYPES,
        maximum_volume_size: 100,
        maximum_iops: 11_800,
        maximum_throughput: 260,
    },
    StorageRow {
        instance_type: "t3.small.search",
        sub_types: GENERAL_PURPOSE_SUB_TYPES,
        maximum_volume_size: 100,
        maximum_iops: 11_800,
        maximum_throughput: 260,
    },
    StorageRow {
        instance_type: "t3.2xlarge.search",
        sub_types: GENERAL_PURPOSE_SUB_TYPES,
        maximum_volume_size: 100,
        maximum_iops: 11_800,
        maximum_throughput: 260,
    },
    StorageRow {
        instance_type: "t4g.small.search",
        sub_types: GENERAL_PURPOSE_SUB_TYPES,
        maximum_volume_size: 100,
        maximum_iops: 11_800,
        maximum_throughput: 260,
    },
    StorageRow {
        instance_type: "t4g.medium.search",
        sub_types: GENERAL_PURPOSE_SUB_TYPES,
        maximum_volume_size: 100,
        maximum_iops: 11_800,
        maximum_throughput: 260,
    },
    StorageRow {
        instance_type: "m5.large.search",
        sub_types: WITH_PROVISIONED_IOPS,
        maximum_volume_size: 512,
        maximum_iops: 16_000,
        maximum_throughput: 1_000,
    },
    StorageRow {
        instance_type: "m5.xlarge.search",
        sub_types: WITH_PROVISIONED_IOPS,
        maximum_volume_size: 1_024,
        maximum_iops: 16_000,
        maximum_throughput: 1_000,
    },
    StorageRow {
        instance_type: "m5.2xlarge.search",
        sub_types: WITH_PROVISIONED_IOPS,
        maximum_volume_size: 2_048,
        maximum_iops: 16_000,
        maximum_throughput: 1_000,
    },
    StorageRow {
        instance_type: "m4.2xlarge.search",
        sub_types: EVERY_SUB_TYPE,
        maximum_volume_size: 2_048,
        maximum_iops: 16_000,
        maximum_throughput: 1_000,
    },
    StorageRow {
        instance_type: "r6g.4xlarge.search",
        sub_types: WITH_PROVISIONED_IOPS,
        maximum_volume_size: 4_096,
        maximum_iops: 16_000,
        maximum_throughput: 1_000,
    },
    StorageRow {
        instance_type: "m5.24xlarge.search",
        sub_types: WITH_PROVISIONED_IOPS,
        maximum_volume_size: 24_576,
        maximum_iops: 16_000,
        maximum_throughput: 1_000,
    },
];

/// `DescribeInstanceTypeLimits`:
/// `GET /2021-01-01/opensearch/instanceTypeLimits/{EngineVersion}/{InstanceType}`, with
/// `query` as its one `@httpQuery` member (empty for a request that sends none).
pub(crate) async fn describe_limits(
    emulator: &Emulator,
    engine_version: &str,
    instance_type: &str,
    query: &str,
) -> Answer {
    let uri = format!("{PREFIX}/opensearch/instanceTypeLimits/{engine_version}/{instance_type}");
    let uri = if query.is_empty() {
        uri
    } else {
        format!("{uri}?{query}")
    };
    emulator.call("GET", &uri, None).await
}

/// The `LimitsByRole` map of a successful answer, with the case named on failure.
pub(crate) fn limits_by_role(answer: &Answer, case: &str) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected HTTP 200, got {} {:?}",
        answer.status, answer.body
    );
    let map = &answer.body["LimitsByRole"];
    assert!(
        map.is_object(),
        "case {case}: expected a LimitsByRole map, got {:?}",
        answer.body
    );
    map.clone()
}

/// The role keys of one answer, as a set.
fn role_keys(map: &Value) -> BTreeSet<String> {
    map.as_object()
        .map(|entries| entries.keys().cloned().collect())
        .unwrap_or_default()
}

/// One role's instance-count range.
fn instance_counts(map: &Value, role: &str, case: &str) -> (i64, i64) {
    let counts = &map[role]["InstanceLimits"]["InstanceCountLimits"];
    let minimum = counts["MinimumInstanceCount"].as_i64().unwrap_or_else(|| {
        panic!("case {case}: expected a MinimumInstanceCount for {role}, got {counts:?}")
    });
    let maximum = counts["MaximumInstanceCount"].as_i64().unwrap_or_else(|| {
        panic!("case {case}: expected a MaximumInstanceCount for {role}, got {counts:?}")
    });
    (minimum, maximum)
}

/// One named limit of one storage type, as the single value it publishes; `None` when
/// the storage type publishes no limit of that name.
fn storage_limit(storage_type: &Value, name: &str) -> Option<i64> {
    storage_type["StorageTypeLimits"]
        .as_array()?
        .iter()
        .find(|limit| limit["LimitName"] == json!(name))
        .and_then(|limit| limit["LimitValues"][0].as_str()?.parse::<i64>().ok())
}

/// One named additional limit of one role, as the single value it publishes.
fn additional_limit(map: &Value, role: &str, name: &str) -> Option<i64> {
    map[role]["AdditionalLimits"]
        .as_array()?
        .iter()
        .find(|limit| limit["LimitName"] == json!(name))
        .and_then(|limit| limit["LimitValues"][0].as_str()?.parse::<i64>().ok())
}

#[tokio::test]
async fn the_answer_carries_one_entry_per_role_the_instance_type_serves() {
    let emulator = Emulator::new();

    // One row per role rule: a general-purpose, a burstable, a compute- and a
    // memory-optimized type that are both data and master eligible; the
    // OpenSearch-optimized family, which is a data node only; and the UltraWarm family,
    // whose key is the one `LimitsByRole`'s own documentation gives the warm tier.
    let rows: [(&str, &[&str]); 7] = [
        ("m5.large.search", &["data", "master"]),
        ("t3.small.search", &["data", "master"]),
        ("c5.9xlarge.search", &["data", "master"]),
        ("r6g.2xlarge.search", &["data", "master"]),
        ("i3.large.search", &["data", "master"]),
        ("or1.4xlarge.search", &["data"]),
        ("ultrawarm1.medium.search", &["ultra_warm"]),
    ];

    for version in VERSIONS {
        for (instance_type, expected) in rows {
            // Only the releases that offer the family answer it; the pairings a release
            // does not offer are the refusal case of their own test below.
            if instance_type.starts_with("or1.") && version != "OpenSearch_2.19" {
                continue;
            }
            let case = format!("{version} {instance_type}");
            let answer = describe_limits(&emulator, version, instance_type, "").await;
            let map = limits_by_role(&answer, &case);
            let expected: BTreeSet<String> =
                expected.iter().map(|role| (*role).to_string()).collect();
            assert_eq!(
                role_keys(&map),
                expected,
                "case {case}: expected limits for exactly the roles the instance type \
                 serves, got {map:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_instance_count_range_is_the_roles_own() {
    let emulator = Emulator::new();
    let version = "OpenSearch_2.19";

    // The suite's own expectation of each role's published range: a data tier is one
    // node or more up to the published per-type ceiling, a master quorum is two to
    // five, and the warm tier is attached in a pair or more.
    let expected: [(&str, (i64, i64)); 3] = [
        ("data", (1, 20)),
        ("master", (2, 5)),
        ("ultra_warm", (2, 150)),
    ];

    for instance_type in [
        "m5.large.search",
        "m5.24xlarge.search",
        "t3.nano.search",
        "r6g.12xlarge.search",
        "or1.medium.search",
        "ultrawarm1.xlarge.search",
    ] {
        let case = format!("{version} {instance_type}");
        let map = limits_by_role(
            &describe_limits(&emulator, version, instance_type, "").await,
            &case,
        );
        for role in role_keys(&map) {
            let (minimum, maximum) = instance_counts(&map, &role, &case);
            let (_, (expected_minimum, expected_maximum)) = expected
                .iter()
                .find(|(name, _)| *name == role)
                .unwrap_or_else(|| panic!("case {case}: unexpected role {role}"));
            assert_eq!(
                (minimum, maximum),
                (*expected_minimum, *expected_maximum),
                "case {case} role {role}: expected the role's published count range, got \
                 ({minimum}, {maximum})"
            );
            assert!(
                minimum >= 1 && maximum >= minimum,
                "case {case} role {role}: expected a non-empty range of at least one \
                 node, got ({minimum}, {maximum})"
            );
        }
        // A data tier scales further than a master quorum, whatever the instance type.
        if role_keys(&map).contains("master") {
            let (_, data_maximum) = instance_counts(&map, "data", &case);
            let (_, master_maximum) = instance_counts(&map, "master", &case);
            assert!(
                data_maximum > master_maximum,
                "case {case}: expected more data nodes than master nodes, got \
                 {data_maximum} and {master_maximum}"
            );
        }
    }
}

#[tokio::test]
async fn ebs_storage_types_are_the_instance_types_own_sub_types_and_ceilings() {
    let emulator = Emulator::new();
    let version = "OpenSearch_2.19";

    for row in &STORAGE_ROWS {
        let StorageRow {
            instance_type,
            sub_types: expected_sub_types,
            maximum_volume_size: expected_maximum,
            maximum_iops: expected_maximum_iops,
            maximum_throughput: expected_maximum_throughput,
        } = *row;
        let case = format!("{version} {instance_type}");
        let map = limits_by_role(
            &describe_limits(&emulator, version, instance_type, "").await,
            &case,
        );
        for role in role_keys(&map) {
            // A volume attaches to the domain's data nodes, so the storage limits are
            // the data role's: no other role publishes them.
            if role != "data" {
                assert!(
                    map[&role]["StorageTypes"].is_null(),
                    "case {case} role {role}: expected no StorageTypes for a role that \
                     attaches no volume, got {:?}",
                    map[&role]
                );
                continue;
            }
            let storage = map[&role]["StorageTypes"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case} role {role}: expected a StorageTypes list, got {:?}",
                        map[&role]
                    )
                })
                .clone();
            let sub_types: Vec<String> = storage
                .iter()
                .map(|entry| entry["StorageSubTypeName"].as_str().unwrap_or("").to_string())
                .collect();
            assert_eq!(
                sub_types,
                expected_sub_types
                    .iter()
                    .map(|sub_type| (*sub_type).to_string())
                    .collect::<Vec<_>>(),
                "case {case} role {role}: expected one ebs entry per volume sub-type this \
                 instance type attaches, got {storage:?}"
            );
            for sub_type in &sub_types {
                assert!(
                    VOLUME_SUB_TYPES.contains(&sub_type.as_str()),
                    "case {case} role {role}: expected a modelled volume sub-type, got \
                     {sub_type}"
                );
            }
            for entry in &storage {
                let sub_type = entry["StorageSubTypeName"].as_str().unwrap_or_default();
                let case = format!("{case} role {role} {sub_type}");
                assert_eq!(
                    entry["StorageTypeName"],
                    json!("ebs"),
                    "case {case}: expected the ebs storage type, got {entry:?}"
                );
                assert_eq!(
                    storage_limit(entry, "MinimumVolumeSize"),
                    Some(MINIMUM_VOLUME_SIZE),
                    "case {case}: expected the published volume-size floor, got {entry:?}"
                );
                let expected_maximum = if sub_type == "standard" {
                    expected_maximum.min(MAXIMUM_STANDARD_VOLUME_SIZE)
                } else {
                    expected_maximum
                };
                assert_eq!(
                    storage_limit(entry, "MaximumVolumeSize"),
                    Some(expected_maximum),
                    "case {case}: expected the maximum this instance size publishes, got \
                     {entry:?}"
                );
                assert!(
                    storage_limit(entry, "MaximumVolumeSize") >= Some(MINIMUM_VOLUME_SIZE),
                    "case {case}: expected a maximum at or above the floor, got {entry:?}"
                );

                // IOPS are provisioned per volume for `gp3` and the provisioned-IOPS
                // type only, and throughput for `gp3` only; every other sub-type
                // publishes neither, rather than a range of nothing.
                let iops = matches!(sub_type, "gp3" | "io1");
                assert_eq!(
                    storage_limit(entry, "MinimumIops").is_some(),
                    iops,
                    "case {case}: expected an IOPS floor only where IOPS are \
                     provisioned, got {entry:?}"
                );
                assert_eq!(
                    storage_limit(entry, "MaximumIops").is_some(),
                    iops,
                    "case {case}: expected an IOPS ceiling only where IOPS are \
                     provisioned, got {entry:?}"
                );
                assert_eq!(
                    storage_limit(entry, "MinimumThroughput").is_some(),
                    sub_type == "gp3",
                    "case {case}: expected a throughput floor only where throughput is \
                     provisioned, got {entry:?}"
                );
                if iops {
                    let floor = storage_limit(entry, "MinimumIops").unwrap_or_default();
                    let ceiling = storage_limit(entry, "MaximumIops").unwrap_or_default();
                    assert!(
                        floor > 0 && ceiling > floor,
                        "case {case}: expected a non-empty IOPS range, got ({floor}, \
                         {ceiling})"
                    );
                    // The ceiling is what the INSTANCE drives, not what the sub-type
                    // alone publishes.
                    assert_eq!(
                        ceiling, expected_maximum_iops,
                        "case {case}: expected the IOPS ceiling this instance type \
                         publishes, got {ceiling}"
                    );
                }
                if sub_type == "gp3" {
                    let floor = storage_limit(entry, "MinimumThroughput").unwrap_or_default();
                    let ceiling = storage_limit(entry, "MaximumThroughput").unwrap_or_default();
                    assert!(
                        floor > 0 && ceiling > floor,
                        "case {case}: expected a non-empty throughput range, got \
                         ({floor}, {ceiling})"
                    );
                    assert_eq!(
                        ceiling, expected_maximum_throughput,
                        "case {case}: expected the throughput ceiling this instance type \
                         publishes, got {ceiling}"
                    );
                }
            }
        }
    }

    // A bigger instance takes a bigger volume, up to what its own attachment carries:
    // the maximum never falls along the table, and the largest instance publishes a
    // bigger maximum than the smallest.
    let mut previous: Option<(&str, i64)> = None;
    for row in &STORAGE_ROWS {
        if let Some((smaller, smaller_maximum)) = previous {
            assert!(
                row.maximum_volume_size >= smaller_maximum,
                "case {smaller} -> {}: expected the published maximum to grow with the \
                 instance size, got {smaller_maximum} and {}",
                row.instance_type,
                row.maximum_volume_size
            );
        }
        previous = Some((row.instance_type, row.maximum_volume_size));
    }
    let (smallest, largest) = (
        STORAGE_ROWS[0].maximum_volume_size,
        STORAGE_ROWS[STORAGE_ROWS.len() - 1].maximum_volume_size,
    );
    assert!(
        largest > smallest,
        "case the size table: expected the largest instance to publish a bigger maximum \
         than the smallest, got {largest} and {smallest}"
    );
}

#[tokio::test]
async fn built_in_storage_publishes_no_volume_limits() {
    let emulator = Emulator::new();
    let version = "OpenSearch_2.19";

    // One row per family whose data sits on storage built into the instance, over three
    // size classes, so the rule is the family's and not one type's.
    for instance_type in [
        "i3.large.search",
        "i3.8xlarge.search",
        "i2.xlarge.search",
        "d2.4xlarge.search",
        "r3.large.search",
        "r6gd.12xlarge.search",
    ] {
        let case = format!("{version} {instance_type}");
        let map = limits_by_role(
            &describe_limits(&emulator, version, instance_type, "").await,
            &case,
        );
        for role in role_keys(&map) {
            if role != "data" {
                assert!(
                    map[&role]["StorageTypes"].is_null(),
                    "case {case} role {role}: expected no StorageTypes for a role that \
                     attaches no volume, got {:?}",
                    map[&role]
                );
                continue;
            }
            let storage = map[&role]["StorageTypes"].as_array().cloned().unwrap_or_default();
            assert_eq!(
                storage.len(),
                1,
                "case {case} role {role}: expected exactly the built-in storage type, \
                 got {storage:?}"
            );
            assert_eq!(
                storage[0]["StorageTypeName"],
                json!("instance"),
                "case {case} role {role}: expected the instance storage type, got {:?}",
                storage[0]
            );
            assert!(
                storage[0]["StorageSubTypeName"].is_null()
                    && storage[0]["StorageTypeLimits"].is_null(),
                "case {case} role {role}: expected built-in storage to publish no \
                 sub-type and no volume limit, got {:?}",
                storage[0]
            );
        }
    }

    // The warm tier's storage is not attached to the node at all, so a warm instance
    // type publishes no storage type rather than an empty list of them.
    for instance_type in [
        "ultrawarm1.medium.search",
        "ultrawarm1.large.search",
        "ultrawarm1.xlarge.search",
    ] {
        let case = format!("{version} {instance_type}");
        let map = limits_by_role(
            &describe_limits(&emulator, version, instance_type, "").await,
            &case,
        );
        assert!(
            map["ultra_warm"]["StorageTypes"].is_null(),
            "case {case}: expected no StorageTypes member, got {:?}",
            map["ultra_warm"]
        );
        assert!(
            map["ultra_warm"]["AdditionalLimits"].is_null(),
            "case {case}: expected no AdditionalLimits member for a role neither \
             published additional limit applies to, got {:?}",
            map["ultra_warm"]
        );
    }
}

#[tokio::test]
async fn the_additional_limit_is_the_roles_own() {
    let emulator = Emulator::new();
    let version = "OpenSearch_2.19";

    // The data nodes a dedicated master supports, by the size of the master: the
    // suite's own expectation of the published figures, which grow with the master.
    let rows: [(&str, i64); 5] = [
        ("m5.large.search", 10),
        ("m5.xlarge.search", 30),
        ("m5.2xlarge.search", 75),
        ("m5.4xlarge.search", 125),
        ("m5.24xlarge.search", 125),
    ];

    let mut previous: Option<(&str, i64)> = None;
    for (instance_type, supported) in rows {
        let case = format!("{version} {instance_type}");
        let map = limits_by_role(
            &describe_limits(&emulator, version, instance_type, "").await,
            &case,
        );
        assert_eq!(
            additional_limit(&map, "master", "MaximumNumberOfDataNodesSupported"),
            Some(supported),
            "case {case}: expected the data nodes this master size supports, got {:?}",
            map["master"]["AdditionalLimits"]
        );
        // The data-node limit is a data-node one and the master limit a master one, so
        // neither appears under the other's role.
        assert_eq!(
            additional_limit(&map, "master", "MaximumNumberOfDataNodesWithoutMasterNode"),
            None,
            "case {case}: expected no data-node limit under the master role, got {:?}",
            map["master"]["AdditionalLimits"]
        );
        assert_eq!(
            additional_limit(&map, "data", "MaximumNumberOfDataNodesSupported"),
            None,
            "case {case}: expected no master limit under the data role, got {:?}",
            map["data"]["AdditionalLimits"]
        );
        assert_eq!(
            additional_limit(&map, "data", "MaximumNumberOfDataNodesWithoutMasterNode"),
            Some(10),
            "case {case}: expected the data nodes a domain runs without a master, got \
             {:?}",
            map["data"]["AdditionalLimits"]
        );
        if let Some((smaller, smaller_supported)) = previous {
            assert!(
                supported >= smaller_supported,
                "case {smaller} -> {instance_type}: expected a bigger master to govern \
                 at least as many data nodes, got {smaller_supported} and {supported}"
            );
        }
        previous = Some((instance_type, supported));
    }
}

#[tokio::test]
async fn a_pairing_the_release_does_not_offer_is_refused() {
    let emulator = Emulator::new();

    // One row per published engine-release prerequisite, each asked on a release just
    // before it: the UltraWarm family, the Graviton families and the OpenSearch-only
    // family (asked on the last Elasticsearch release and on the OpenSearch release
    // before it).
    for (version, instance_type) in [
        ("Elasticsearch_6.7", "ultrawarm1.medium.search"),
        ("Elasticsearch_7.8", "c6g.large.search"),
        ("Elasticsearch_7.8", "r6g.2xlarge.search"),
        ("Elasticsearch_7.8", "t4g.small.search"),
        ("Elasticsearch_7.10", "or1.large.search"),
        ("OpenSearch_2.9", "or1.large.search"),
    ] {
        let case = format!("{version} {instance_type}");
        let answer = describe_limits(&emulator, version, instance_type, "").await;
        assert_eq!(
            answer.status, 409,
            "case {case}: expected HTTP 409, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "InvalidTypeException",
            "case {case}: expected the modelled unsupported-type refusal, got {:?}",
            answer.body
        );
    }

    // The release that introduces each family answers the same pairing.
    for (version, instance_type) in [
        ("Elasticsearch_6.8", "ultrawarm1.medium.search"),
        ("Elasticsearch_7.9", "c6g.large.search"),
        ("OpenSearch_2.11", "or1.large.search"),
    ] {
        let case = format!("{version} {instance_type}");
        let answer = describe_limits(&emulator, version, instance_type, "").await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected the release that offers the family to answer, got {} \
             {:?}",
            answer.status, answer.body
        );
    }
}

#[tokio::test]
async fn a_request_naming_no_release_no_type_or_no_domain_is_refused() {
    let emulator = Emulator::new();
    let version = "OpenSearch_2.11";
    let instance_type = "m5.large.search";

    // An `EngineVersion` of the member's own length that names no engine release: the
    // limits are a function of the release.
    for malformed in [
        "Opensearch_2.11",
        "OpenSearch_abc",
        "Elasticsearch_x.y",
        "AmazonSearch_2.11",
    ] {
        let answer = describe_limits(&emulator, malformed, instance_type, "").await;
        assert_eq!(
            answer.status, 400,
            "case {malformed}: expected HTTP 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {malformed}: expected the modelled validation refusal, got {:?}",
            answer.body
        );
    }

    // A value of the instance-type shape that the service publishes no instance type
    // for names nothing at all.
    for unknown in [
        "m5.enormous.search",
        "zz9.large.search",
        "m5.large.domain",
        "notaninstancetype",
    ] {
        let answer = describe_limits(&emulator, version, unknown, "").await;
        assert_eq!(
            answer.status, 400,
            "case {unknown}: expected HTTP 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {unknown}: expected the modelled validation refusal, got {:?}",
            answer.body
        );
    }

    // "Only specify if you need the limits for an existing domain" — a `domainName` the
    // account holds no domain for names no resource, over a table of names inside the
    // member's own length bounds.
    for absent in ["nosuch", "absent-domain", "zzz"] {
        let answer =
            describe_limits(&emulator, version, instance_type, &format!("domainName={absent}"))
                .await;
        assert_eq!(
            answer.status, RECORDED_NOT_FOUND_STATUS,
            "case {absent}: expected HTTP {RECORDED_NOT_FOUND_STATUS}, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {absent}: expected the modelled not-found refusal, got {:?}",
            answer.body
        );
    }

    // A deleted domain holds no record either, so the same refusal follows a delete.
    let name = "limits-domain";
    let created = emulator.create(json!({ "DomainName": name })).await;
    assert_eq!(
        created.status, 200,
        "case create {name}: expected HTTP 200, got {} {:?}",
        created.status, created.body
    );
    let scoped =
        describe_limits(&emulator, version, instance_type, &format!("domainName={name}")).await;
    let unscoped = describe_limits(&emulator, version, instance_type, "").await;
    assert_eq!(
        limits_by_role(&scoped, "existing domain"),
        limits_by_role(&unscoped, "no domain"),
        "case {name}: expected a published catalog, so naming a domain reports the same \
         limits as naming none"
    );
    emulator
        .call("DELETE", &format!("{PREFIX}/opensearch/domain/{name}"), None)
        .await;
    let after =
        describe_limits(&emulator, version, instance_type, &format!("domainName={name}")).await;
    assert_eq!(
        after.status, RECORDED_NOT_FOUND_STATUS,
        "case deleted domain: expected HTTP {RECORDED_NOT_FOUND_STATUS}, got {} {:?}",
        after.status, after.body
    );
}

#[tokio::test]
async fn reading_the_limits_changes_nothing() {
    let emulator = Emulator::new();
    let version = "OpenSearch_2.11";
    let instance_type = "r6g.large.search";

    let name = "untouched-domain";
    emulator
        .create(json!({
            "DomainName": name,
            "ClusterConfig": { "InstanceType": instance_type, "InstanceCount": 2 },
            "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 20 },
        }))
        .await;
    let before = emulator.describe(name).await;
    let first = limits_by_role(
        &describe_limits(&emulator, version, instance_type, "").await,
        "first read",
    );

    // Reads of every shape this operation answers, including the refused ones.
    describe_limits(&emulator, version, instance_type, &format!("domainName={name}")).await;
    describe_limits(&emulator, version, "ultrawarm1.medium.search", "").await;
    describe_limits(&emulator, "Elasticsearch_7.8", "c6g.large.search", "").await;
    describe_limits(&emulator, version, instance_type, "domainName=nosuch").await;

    let second = limits_by_role(
        &describe_limits(&emulator, version, instance_type, "").await,
        "second read",
    );
    assert_eq!(
        first, second,
        "case {instance_type}: expected two reads of one instance type to report the \
         same limits"
    );
    let after = emulator.describe(name).await;
    assert_eq!(
        before.body, after.body,
        "case {name}: expected the domain record to be untouched by a limit read"
    );
}

/// The published volume-size range of one instance type and sub-type — its
/// `MinimumVolumeSize` and `MaximumVolumeSize` — read through
/// `DescribeInstanceTypeLimits` itself, so the enforcement cases below are bounded by
/// what the service publishes rather than by a second copy of the scale.
///
/// Shared with [`super::ebs_volume_size_limit`], which holds the refusals to these same
/// published bounds.
pub(crate) async fn published_volume_size_range(
    emulator: &Emulator,
    engine_version: &str,
    instance_type: &str,
    sub_type: &str,
) -> (i64, i64) {
    let case = format!("{engine_version} {instance_type} {sub_type}");
    let map = limits_by_role(
        &describe_limits(emulator, engine_version, instance_type, "").await,
        &case,
    );
    let storage = map["data"]["StorageTypes"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let entry = storage
        .iter()
        .find(|entry| entry["StorageSubTypeName"] == json!(sub_type))
        .unwrap_or_else(|| {
            panic!("case {case}: expected a {sub_type} storage type, got {storage:?}")
        });
    let minimum = storage_limit(entry, "MinimumVolumeSize")
        .unwrap_or_else(|| panic!("case {case}: expected a MinimumVolumeSize, got {entry:?}"));
    let maximum = storage_limit(entry, "MaximumVolumeSize")
        .unwrap_or_else(|| panic!("case {case}: expected a MaximumVolumeSize, got {entry:?}"));
    (minimum, maximum)
}

#[tokio::test]
async fn a_volume_outside_the_published_range_is_refused_on_create() {
    let emulator = Emulator::new();
    let version = "OpenSearch_2.11";

    // One row per instance size and sub-type the published range differs for, each
    // probed at both ends of its range and one past each end: the floor, the floor less
    // one, the published maximum, and one over it.
    for (row, (instance_type, sub_type)) in [
        ("m5.large.search", "gp3"),
        ("m5.2xlarge.search", "gp2"),
        ("m4.2xlarge.search", "io1"),
        ("m4.large.search", "standard"),
        ("t3.nano.search", "gp3"),
    ]    .into_iter()
    .enumerate()
    {
        let (_, maximum) =
            published_volume_size_range(&emulator, version, instance_type, sub_type).await;
        let rows: [(i64, bool); 5] = [
            (MINIMUM_VOLUME_SIZE - 1, false),
            (1, false),
            (MINIMUM_VOLUME_SIZE, true),
            (maximum, true),
            (maximum + 1, false),
        ];
        for (index, (size, accepted)) in rows.into_iter().enumerate() {
            // A distinct domain per case, so a row is never answered by an earlier
            // row's record; the name carries the row and the probe it is for.
            let name = format!("ebs-{row}-{index}-{sub_type}");
            let case = format!("{instance_type} {sub_type} {size} GiB");
            let answer = emulator
                .create(json!({
                    "DomainName": name,
                    "ClusterConfig": { "InstanceType": instance_type, "InstanceCount": 1 },
                    "EBSOptions": {
                        "EBSEnabled": true,
                        "VolumeType": sub_type,
                        "VolumeSize": size,
                    },
                }))
                .await;
            if accepted {
                assert_eq!(
                    answer.status, 200,
                    "case {case}: expected a size the service publishes to be accepted, \
                     got {} {:?}",
                    answer.status, answer.body
                );
                // Round trip: the accepted volume is what a later read reports.
                let stored = emulator.describe(&name).await;
                assert_eq!(
                    stored.body["DomainStatus"]["EBSOptions"]["VolumeSize"],
                    json!(size),
                    "case {case}: expected the stored volume size, got {:?}",
                    stored.body["DomainStatus"]["EBSOptions"]
                );
            } else {
                // A size outside the published range asks for storage the service does
                // not offer for that instance type: the modelled service-limit refusal,
                // not a request-shape one.
                assert_eq!(
                    answer.status, 409,
                    "case {case}: expected HTTP 409, got {} {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    error_code(&answer),
                    "LimitExceededException",
                    "case {case}: expected the modelled service-limit refusal, got {:?}",
                    answer.body
                );
                // A refused create stores nothing.
                let stored = emulator.describe(&name).await;
                assert_eq!(
                    stored.status, RECORDED_NOT_FOUND_STATUS,
                    "case {case}: expected no domain to have been created, got {} {:?}",
                    stored.status, stored.body
                );
            }
        }
    }

    // EBS not enabled attaches no volume, so there is no volume to size: the size the
    // request carries sizes nothing and the create is accepted.
    let answer = emulator
        .create(json!({
            "DomainName": "ebs-disabled",
            "ClusterConfig": { "InstanceType": "m5.large.search", "InstanceCount": 1 },
            "EBSOptions": { "EBSEnabled": false, "VolumeSize": 1 },
        }))
        .await;
    assert_eq!(
        answer.status, 200,
        "case EBS not enabled: expected no volume to be sized, got {} {:?}",
        answer.status, answer.body
    );
}

#[tokio::test]
async fn a_volume_outside_the_published_range_is_refused_on_update() {
    let emulator = Emulator::new();
    let version = "OpenSearch_2.11";
    let instance_type = "m5.large.search";
    let name = "ebs-resize";
    let (_, maximum) =
        published_volume_size_range(&emulator, version, instance_type, "gp3").await;

    let created = emulator
        .create(json!({
            "DomainName": name,
            "ClusterConfig": { "InstanceType": instance_type, "InstanceCount": 1 },
            "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 20 },
        }))
        .await;
    assert_eq!(
        created.status, 200,
        "case create {name}: expected HTTP 200, got {} {:?}",
        created.status, created.body
    );
    // The service applies one change of a domain at a time, so the change the create
    // started has to land before an update can start another.
    tokio::time::sleep(SETTLE_WAIT).await;

    // One row per way a merged configuration leaves the published range: a resize below
    // the floor, a resize over the instance's maximum, a magnetic volume over the flat
    // cap, and — the other half of the merge — an instance type too small for the
    // volume the request keeps.
    let refused: [(&str, Value); 4] = [
        (
            "resize below the floor",
            json!({ "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 5 } }),
        ),
        (
            "resize over the maximum",
            json!({
                "EBSOptions": {
                    "EBSEnabled": true,
                    "VolumeType": "gp3",
                    "VolumeSize": maximum + 1,
                }
            }),
        ),
        (
            "magnetic over the flat cap",
            json!({
                "EBSOptions": {
                    "EBSEnabled": true,
                    "VolumeType": "standard",
                    "VolumeSize": MAXIMUM_STANDARD_VOLUME_SIZE + 1,
                }
            }),
        ),
        (
            "a volume over the maximum of the instance type the request names",
            json!({
                "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 600 },
                "ClusterConfig": { "InstanceType": "t3.small.search", "InstanceCount": 1 },
            }),
        ),
    ];

    for (case, body) in refused {
        let answer = emulator.update_config(name, body.clone()).await;
        assert_eq!(
            answer.status, 409,
            "case {case}: expected HTTP 409, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "LimitExceededException",
            "case {case}: expected the modelled service-limit refusal, got {:?}",
            answer.body
        );
        // A refused update stores nothing: the domain keeps the volume it had.
        let stored = emulator.describe(name).await;
        assert_eq!(
            stored.body["DomainStatus"]["EBSOptions"]["VolumeSize"],
            json!(20),
            "case {case}: expected the stored volume to be untouched, got {:?}",
            stored.body["DomainStatus"]["EBSOptions"]
        );
        // A dry run reports what applying the change would do, so a change that cannot
        // be applied is refused there too.
        let mut dry = body;
        dry["DryRun"] = json!(true);
        let answer = emulator.update_config(name, dry).await;
        assert_eq!(
            answer.status, 409,
            "case {case} dry run: expected HTTP 409, got {} {:?}",
            answer.status, answer.body
        );
    }

    // A resize INSIDE the published range is applied and read back.
    let answer = emulator
        .update_config(
            name,
            json!({
                "EBSOptions": {
                    "EBSEnabled": true,
                    "VolumeType": "gp3",
                    "VolumeSize": maximum,
                }
            }),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case resize to the maximum: expected HTTP 200, got {} {:?}",
        answer.status, answer.body
    );
    let stored = emulator.describe(name).await;
    assert_eq!(
        stored.body["DomainStatus"]["EBSOptions"]["VolumeSize"],
        json!(maximum),
        "case resize to the maximum: expected the new volume size, got {:?}",
        stored.body["DomainStatus"]["EBSOptions"]
    );

    // The other half of the merge: a request that changes only the INSTANCE TYPE is
    // judged against the volume the domain keeps, so shrinking the instance under its
    // volume is refused and growing it is not.
    let wide = "ebs-wide";
    let created = emulator
        .create(json!({
            "DomainName": wide,
            "ClusterConfig": { "InstanceType": "m5.xlarge.search", "InstanceCount": 1 },
            "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 1_000 },
        }))
        .await;
    assert_eq!(
        created.status, 200,
        "case create {wide}: expected HTTP 200, got {} {:?}",
        created.status, created.body
    );
    // The change the create started has to land before an update can start another.
    tokio::time::sleep(SETTLE_WAIT).await;
    let shrunk = emulator
        .update_config(
            wide,
            json!({ "ClusterConfig": { "InstanceType": "m5.large.search", "InstanceCount": 1 } }),
        )
        .await;
    assert_eq!(
        shrunk.status, 409,
        "case shrink the instance under its volume: expected HTTP 409, got {} {:?}",
        shrunk.status, shrunk.body
    );
    let grown = emulator
        .update_config(
            wide,
            json!({ "ClusterConfig": { "InstanceType": "m5.2xlarge.search", "InstanceCount": 1 } }),
        )
        .await;
    assert_eq!(
        grown.status, 200,
        "case grow the instance over its volume: expected HTTP 200, got {} {:?}",
        grown.status, grown.body
    );
}
