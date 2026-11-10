// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the conventions the ElastiCache read-only CATALOGS are
//! published under: the engine-version catalog
//! (`DescribeCacheEngineVersions`), the default cache parameter groups
//! (`DescribeCacheParameterGroups` / `DescribeCacheParameters`), the
//! reserved-cache-node offering catalog
//! (`DescribeReservedCacheNodesOfferings`) and the service-update catalog
//! (`DescribeServiceUpdates`).
//!
//! The rules under test, each asserted over a SET of inputs rather than one
//! recorded answer:
//!
//! * a published engine version is described by the engine's OWN name, and its
//!   version description is that name plus the version;
//! * the catalog publishes only versions the service offers, so a version it
//!   has retired is reported by nothing;
//! * a family that can run cluster mode publishes BOTH its plain default
//!   parameter group and its cluster-mode-on default, which is that same family
//!   default with the cluster-mode parameter turned on — and a cluster-mode
//!   group cannot be used to create a cache cluster;
//! * every offering publishes its hourly recurring charge as a
//!   `RecurringCharge` structure, with the amount the payment option leaves to
//!   be paid hourly;
//! * a published service update's name is fixed by the catalog, so it does not
//!   move with the clock the store came up on.
//!
//! Every assertion goes over the wire through the same router the production
//! emulator mounts (the shared [`crate::harness`]), so only the HTTP status and
//! the XML body are inspected.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::describe_cache_parameters::{describe_ok as describe_parameters, one as one_parameter};
use crate::harness::{params, Emulator, Reply};

/// One engine-naming row: the engine, the description its catalog entries carry
/// and a version of it the catalog publishes.
///
/// The descriptions are the engines' own product names, spelled the way each
/// project spells its own name — `memcached` in lower case, `Redis` and
/// `Valkey` capitalised — which is the convention
/// `CacheEngineVersion$CacheEngineDescription` reports. The VERSION description
/// is built from the engine's API name instead (the `Engine` value itself), as
/// `<engine> version <version>`.
const ENGINE_NAMING: [(&str, &str, &str); 3] = [
    ("memcached", "memcached", "1.4.14"),
    ("redis", "Redis", "7.1"),
    ("valkey", "Valkey", "7.2"),
];

/// One catalog-floor row: the engine, the oldest version the service offers for
/// it, and a version it does not offer at all.
///
/// Source: the engine lines ElastiCache publishes — Memcached starts at 1.4.14
/// (the version `DescribeCacheEngineVersions$EngineVersion`'s own documentation
/// uses as its example), Redis OSS at 2.6.13 and Valkey at 7.2 — so a lower
/// version of the same line (1.4.5, 2.6.0, 7.0) is one no entry reports.
const CATALOG_FLOOR: [(&str, &str, &str); 3] = [
    ("memcached", "1.4.14", "1.4.5"),
    ("redis", "2.6.13", "2.6.0"),
    ("valkey", "7.2", "7.0"),
];

/// One parameter-group row: the family, and whether it publishes a
/// cluster-mode-on default group beside its plain default.
///
/// Source: cluster mode is a Valkey / Redis OSS feature of Redis OSS 3.2 and
/// later (`CreateReplicationGroup`'s own documentation: "To create a Valkey or
/// Redis OSS (cluster mode enabled) replication group, use
/// CacheParameterGroupName=default.redis3.2.cluster.on"), so the families from
/// that line onward publish one and the Memcached and pre-3.2 families — which
/// cannot run cluster mode at all — do not.
const CLUSTER_MODE_BY_FAMILY: [(&str, bool); 12] = [
    ("memcached1.4", false),
    ("memcached1.5", false),
    ("memcached1.6", false),
    ("redis2.6", false),
    ("redis2.8", false),
    ("redis3.2", true),
    ("redis4.0", true),
    ("redis5.0", true),
    ("redis6.x", true),
    ("redis7", true),
    ("valkey7", true),
    ("valkey8", true),
];

/// Every `CacheEngineVersion` a reply reports, as `(engine, version,
/// engine description, version description)` in document order.
fn wire_versions(case: &str, reply: &Reply) -> Vec<(String, String, String, String)> {
    let engines = reply.all("Engine");
    let versions = reply.all("EngineVersion");
    let engine_descriptions = reply.all("CacheEngineDescription");
    let version_descriptions = reply.all("CacheEngineVersionDescription");
    for (member, column) in [
        ("EngineVersion", versions.len()),
        ("CacheEngineDescription", engine_descriptions.len()),
        ("CacheEngineVersionDescription", version_descriptions.len()),
    ] {
        assert_eq!(
            column,
            engines.len(),
            "case {case}: {member} must be reported once per version ({}), got {column}: {}",
            engines.len(),
            reply.body
        );
    }
    (0..engines.len())
        .map(|index| {
            (
                engines[index].to_string(),
                versions[index].to_string(),
                engine_descriptions[index].to_string(),
                version_descriptions[index].to_string(),
            )
        })
        .collect()
}

/// One `DescribeCacheEngineVersions` call that must succeed.
async fn describe_versions(
    emulator: &Emulator,
    case: &str,
    pairs: &[(&str, &str)],
) -> Vec<(String, String, String, String)> {
    let reply = emulator
        .call("DescribeCacheEngineVersions", &params(pairs))
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case}: DescribeCacheEngineVersions must answer 200, got {} {}",
        reply.status, reply.body
    );
    wire_versions(case, &reply)
}

#[tokio::test]
async fn every_published_version_is_described_by_its_engines_own_name() {
    let emulator = Emulator::new();

    // The axis: the engine, over every engine this service runs. Each row's
    // whole version list is asserted, so the rule holds for every version of
    // the engine and not only the one the row names.
    for (engine, description, published_version) in ENGINE_NAMING {
        let rows = describe_versions(&emulator, engine, &[("Engine", engine)]).await;
        assert!(
            !rows.is_empty(),
            "case {engine}: the catalog must publish versions of {engine}"
        );
        assert!(
            rows.iter().any(|(_, version, _, _)| version == published_version),
            "case {engine}: the catalog must publish {engine} {published_version}, got {:?}",
            rows.iter().map(|(_, version, _, _)| version).collect::<Vec<_>>()
        );
        for (reported_engine, version, engine_description, version_description) in &rows {
            let case = format!("{engine} {version}");
            assert_eq!(
                reported_engine, engine,
                "case {case}: the Engine filter must select only {engine}, got {reported_engine:?}"
            );
            assert_eq!(
                engine_description, description,
                "case {case}: expected the engine description {description:?}, got \
                 {engine_description:?}"
            );
            assert_eq!(
                version_description,
                &format!("{engine} version {version}"),
                "case {case}: expected the version description {:?}, got {version_description:?}",
                format!("{engine} version {version}")
            );
        }
    }

    // A read does not change what the next caller sees.
    let once = describe_versions(&emulator, "twice-first", &[]).await;
    let twice = describe_versions(&emulator, "twice-second", &[]).await;
    assert_eq!(
        once, twice,
        "case twice: two reads of a published catalog must report the same versions"
    );
}

#[tokio::test]
async fn the_catalog_publishes_only_versions_the_service_offers() {
    let emulator = Emulator::new();

    for (engine, oldest, retired) in CATALOG_FLOOR {
        let rows = describe_versions(&emulator, engine, &[("Engine", engine)]).await;
        let reported: Vec<&String> = rows.iter().map(|(_, version, _, _)| version).collect();
        assert_eq!(
            reported.first().map(|version| version.as_str()),
            Some(oldest),
            "case {engine}: the catalog is ordered oldest first and starts at {oldest}, got \
             {reported:?}"
        );
        assert!(
            !reported.iter().any(|version| *version == retired),
            "case {engine}: {retired} is not a version this service offers, got {reported:?}"
        );
        // A filter naming a version the catalog does not publish is an empty
        // page rather than a refusal: this operation reports a published
        // catalog, not an account's resources.
        let empty = describe_versions(
            &emulator,
            &format!("{engine} {retired}"),
            &[("Engine", engine), ("EngineVersion", retired)],
        )
        .await;
        assert!(
            empty.is_empty(),
            "case {engine} {retired}: a version nothing publishes selects nothing, got {empty:?}"
        );
    }
}

/// Every `CacheParameterGroupName` a describe reports, with the family,
/// description and ARN beside it, in document order.
fn wire_groups(case: &str, reply: &Reply) -> Vec<(String, String, String, String)> {
    let names = reply.all("CacheParameterGroups/CacheParameterGroup/CacheParameterGroupName");
    let families = reply.all("CacheParameterGroups/CacheParameterGroup/CacheParameterGroupFamily");
    let descriptions = reply.all("CacheParameterGroups/CacheParameterGroup/Description");
    let arns = reply.all("CacheParameterGroups/CacheParameterGroup/ARN");
    for (member, column) in [
        ("CacheParameterGroupFamily", families.len()),
        ("Description", descriptions.len()),
        ("ARN", arns.len()),
    ] {
        assert_eq!(
            column,
            names.len(),
            "case {case}: {member} must be reported once per group ({}), got {column}: {}",
            names.len(),
            reply.body
        );
    }
    (0..names.len())
        .map(|index| {
            (
                names[index].to_string(),
                families[index].to_string(),
                descriptions[index].to_string(),
                arns[index].to_string(),
            )
        })
        .collect()
}

#[tokio::test]
async fn every_cluster_capable_family_publishes_its_cluster_mode_on_default_group() {
    let emulator = Emulator::new();
    let reply = emulator
        .call(
            "DescribeCacheParameterGroups",
            &params(&[("MaxRecords", "100")]),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case describe-all: DescribeCacheParameterGroups must answer 200, got {} {}",
        reply.status, reply.body
    );
    let groups = wire_groups("describe-all", &reply);

    for (family, publishes_cluster_mode) in CLUSTER_MODE_BY_FAMILY {
        let plain = format!("default.{family}");
        let cluster_on = format!("{plain}.cluster.on");
        assert!(
            groups.iter().any(|(name, _, _, _)| *name == plain),
            "case {family}: the plain default group {plain} must be published, got {:?}",
            groups.iter().map(|(name, _, _, _)| name).collect::<Vec<_>>()
        );

        let reported = groups.iter().find(|(name, _, _, _)| *name == cluster_on);
        assert_eq!(
            reported.is_some(),
            publishes_cluster_mode,
            "case {family}: expected cluster-mode-on default published={publishes_cluster_mode}, \
             got {reported:?}"
        );

        if !publishes_cluster_mode {
            // A family that cannot run cluster mode has no such group at all,
            // so naming one is the modelled not-found.
            let missing = emulator
                .call(
                    "DescribeCacheParameterGroups",
                    &params(&[("CacheParameterGroupName", &cluster_on)]),
                )
                .await;
            assert_eq!(
                missing.status, 404,
                "case {family}: {cluster_on} must not exist, got {} {}",
                missing.status, missing.body
            );
            assert_eq!(
                missing.error_code(),
                "CacheParameterGroupNotFound",
                "case {family}: expected CacheParameterGroupNotFound, got {}",
                missing.body
            );
            continue;
        }

        let (_, reported_family, description, arn) =
            reported.expect("a cluster-mode family reports its cluster-mode-on group");
        // The cluster-mode-on default belongs to the SAME family as the plain
        // one: it is that family's defaults with cluster mode turned on.
        assert_eq!(
            reported_family, family,
            "case {family}: {cluster_on} must belong to {family}, got {reported_family:?}"
        );
        assert_eq!(
            description,
            &format!("Customized default parameter group for {family} with cluster mode on"),
            "case {family}: unexpected description for {cluster_on}, got {description:?}"
        );
        assert_eq!(
            arn,
            &format!(
                "arn:aws:elasticache:{}:{}:parametergroup:{cluster_on}",
                crate::harness::REGION,
                engine::emulator_account_id()
            ),
            "case {family}: unexpected ARN for {cluster_on}, got {arn:?}"
        );

        // The one value the two groups differ by, read back through the
        // parameter reader: cluster mode is ON in the cluster-mode-on group and
        // OFF in the plain default. The parameter is one the engine fixes, so
        // either way it is reported as the SERVICE's value rather than a
        // customer's.
        for (group, expected) in [
            (&cluster_on, state::CLUSTER_ENABLED_ON),
            (&plain, "no"),
        ] {
            let case = format!("{group}/{}", state::CLUSTER_ENABLED_PARAMETER);
            let parameters = describe_parameters(&emulator, group, &[]).await;
            let parameter = one_parameter(&parameters, &case, state::CLUSTER_ENABLED_PARAMETER);
            assert_eq!(
                parameter.field("ParameterValue"),
                Some(expected),
                "case {case}: expected {expected:?}, got {:?}",
                parameter.field("ParameterValue")
            );
            assert_eq!(
                parameter.field("Source"),
                Some(state::PARAMETER_SOURCE_SYSTEM),
                "case {case}: a parameter the engine fixes is the service's value, got {:?}",
                parameter.field("Source")
            );
            assert_eq!(
                parameter.field("IsModifiable"),
                Some("false"),
                "case {case}: cluster mode is not a parameter a customer may set, got {:?}",
                parameter.field("IsModifiable")
            );
        }
    }
}

#[tokio::test]
async fn a_cluster_mode_on_default_group_can_be_named_when_creating_a_replication_group() {
    let emulator = Emulator::new();

    // The axis: the family whose cluster-mode-on default the create names. A group's
    // parameters are its family's ENGINE's, so each row runs the engine of the family it
    // names.
    for (index, family) in ["redis7", "redis6.x", "valkey8"].into_iter().enumerate() {
        let group = format!("default.{family}.cluster.on");
        let engine = state::family_engine(family).to_string();
        let id = format!("cluster-mode-{index}");
        let created = emulator
            .call(
                "CreateReplicationGroup",
                &params(&[
                    ("ReplicationGroupId", &id),
                    ("ReplicationGroupDescription", "cluster mode enabled"),
                    ("Engine", &engine),
                    ("CacheParameterGroupName", &group),
                    ("CacheNodeType", "cache.m5.large"),
                    ("NumNodeGroups", "2"),
                    ("ReplicasPerNodeGroup", "1"),
                ]),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {group}: a group the service publishes must be nameable, got {} {}",
            created.status, created.body
        );

        // Read back through the describe: the group the create was given is the
        // one the replication group carries.
        let described = emulator
            .call(
                "DescribeReplicationGroups",
                &params(&[("ReplicationGroupId", &id)]),
            )
            .await;
        assert_eq!(
            described.status, 200,
            "case {group}: DescribeReplicationGroups must answer 200, got {} {}",
            described.status, described.body
        );
        assert_eq!(
            described.text("ReplicationGroupId"),
            Some(id.as_str()),
            "case {group}: expected the created group {id}, got {}",
            described.body
        );
        assert_eq!(
            described.text("ClusterEnabled"),
            Some("true"),
            "case {group}: a group with more than one shard runs cluster mode, got {}",
            described.body
        );
    }
}

#[tokio::test]
async fn a_cluster_mode_on_default_group_cannot_be_used_to_create_a_cluster() {
    let emulator = Emulator::new();

    // The axis: the parameter group the create names. `CacheParameterGroupName`
    // documents "you cannot use any parameter group which has
    // cluster-enabled='yes' when creating a cluster", so a family's plain
    // default is accepted and its cluster-mode-on default is refused — a
    // cluster-mode group belongs to a replication group. Each row runs the ENGINE of the
    // family it names, because a group's parameters are that engine's.
    let cases: [(&str, &str, &str, u16); 6] = [
        ("plain redis7", "redis", "default.redis7", 200),
        ("cluster-mode redis7", "redis", "default.redis7.cluster.on", 400),
        ("plain valkey8", "valkey", "default.valkey8", 200),
        (
            "cluster-mode valkey8",
            "valkey",
            "default.valkey8.cluster.on",
            400,
        ),
        ("plain redis3.2", "redis", "default.redis3.2", 200),
        (
            "cluster-mode redis3.2",
            "redis",
            "default.redis3.2.cluster.on",
            400,
        ),
    ];

    for (index, (case, engine, group, status)) in cases.into_iter().enumerate() {
        let id = format!("param-group-{index}");
        let reply = emulator
            .call(
                "CreateCacheCluster",
                &params(&[
                    ("CacheClusterId", &id),
                    ("Engine", engine),
                    ("CacheNodeType", "cache.m5.large"),
                    ("NumCacheNodes", "1"),
                    ("CacheParameterGroupName", group),
                ]),
            )
            .await;
        assert_eq!(
            reply.status, status,
            "case {case}: expected HTTP {status}, got {} {}",
            reply.status, reply.body
        );
        if status == 200 {
            // The accepted group is the one the cluster carries.
            let described = emulator
                .call("DescribeCacheClusters", &params(&[("CacheClusterId", &id)]))
                .await;
            assert_eq!(
                described.text("CacheParameterGroupName"),
                Some(group),
                "case {case}: the cluster must carry {group}, got {}",
                described.body
            );
        } else {
            assert_eq!(
                reply.error_code(),
                "InvalidParameterValue",
                "case {case}: expected InvalidParameterValue, got {}",
                reply.body
            );
        }
    }
}

/// The numbers reported for the element at `suffix`, in document order.
fn numbers(case: &str, reply: &Reply, suffix: &str) -> Vec<f64> {
    reply
        .all(suffix)
        .into_iter()
        .map(|text| {
            text.parse()
                .unwrap_or_else(|err| panic!("case {case}: {suffix} {text:?} is not a number: {err}"))
        })
        .collect()
}

#[tokio::test]
async fn every_offering_publishes_its_hourly_recurring_charge() {
    let emulator = Emulator::new();

    // Each row is one payment option, with what the option says about the two
    // prices: the share of the term paid up front, and the share left to be
    // paid hourly. Every row publishes the recurring charge as a structure —
    // the amount is what the option changes, not whether a client can read one.
    let cases: [(&str, bool, bool); 3] = [
        (state::OFFERING_TYPE_ALL_UPFRONT, true, false),
        (state::OFFERING_TYPE_PARTIAL_UPFRONT, true, true),
        (state::OFFERING_TYPE_NO_UPFRONT, false, true),
    ];

    for (offering_type, fixed_price_charged, recurring_charged) in cases {
        // One engine at a time keeps the whole answer inside one page, so the
        // prices in the body belong to the offerings in the same body.
        for product_description in state::ENGINES {
            let case = format!("{offering_type} / {product_description}");
            let reply = emulator
                .call(
                    "DescribeReservedCacheNodesOfferings",
                    &params(&[
                        ("OfferingType", offering_type),
                        ("ProductDescription", product_description),
                        ("MaxRecords", "100"),
                    ]),
                )
                .await;
            assert_eq!(
                reply.status, 200,
                "case {case}: describe must answer 200, got {} {}",
                reply.status, reply.body
            );
            let offerings = reply.all("ReservedCacheNodesOfferingId").len();
            assert!(
                offerings > 0,
                "case {case}: the catalog stocks this payment option, got {}",
                reply.body
            );

            // The recurring charge is read through its NESTED path, so an
            // offering whose `RecurringCharges` is not a `RecurringCharge`
            // structure cannot pass.
            let amounts = numbers(
                &case,
                &reply,
                "RecurringCharges/RecurringCharge/RecurringChargeAmount",
            );
            let frequencies = reply.all("RecurringCharges/RecurringCharge/RecurringChargeFrequency");
            assert_eq!(
                amounts.len(),
                offerings,
                "case {case}: every offering must publish one recurring charge, got {amounts:?}"
            );
            assert_eq!(
                frequencies.len(),
                offerings,
                "case {case}: every recurring charge must publish its frequency, got \
                 {frequencies:?}"
            );
            for frequency in &frequencies {
                assert_eq!(
                    *frequency,
                    state::RECURRING_CHARGE_HOURLY,
                    "case {case}: a reservation's recurring charge is hourly, got {frequency:?}"
                );
            }
            for amount in &amounts {
                assert_eq!(
                    *amount > 0.0, recurring_charged,
                    "case {case}: expected an hourly amount charged={recurring_charged}, got \
                     {amount}"
                );
            }
            let fixed_prices = numbers(&case, &reply, "FixedPrice");
            assert_eq!(
                fixed_prices.len(),
                offerings,
                "case {case}: every offering must publish a FixedPrice, got {fixed_prices:?}"
            );
            for (index, price) in fixed_prices.iter().enumerate() {
                assert_eq!(
                    *price > 0.0, fixed_price_charged,
                    "case {case}: expected a fixed price charged={fixed_price_charged} for \
                     offering {index}, got {price}"
                );
                // The pairing the two members have to keep: what is not paid up
                // front is the hourly charge, so exactly one of the two is zero
                // for the all-upfront and no-upfront options.
                assert!(
                    *price > 0.0 || amounts[index] > 0.0,
                    "case {case}: offering {index} must charge something, got fixed {price} and \
                     hourly {}",
                    amounts[index]
                );
            }
        }
    }
}

#[tokio::test]
async fn published_service_update_names_do_not_move_with_the_clock() {
    // The axis: the instant the catalog is built at, spanning years either side
    // of now. The identities a catalog publishes must not depend on it; the
    // DATES must.
    let now = SystemTime::now();
    let year = Duration::from_secs(365 * 24 * 60 * 60);
    let anchors = [
        ("now", now),
        ("a year ago", now - year),
        ("four years ago", now - 4 * year),
        ("a year from now", now + year),
    ];

    let expected: BTreeSet<String> = state::published_service_updates(now).keys().cloned().collect();
    assert!(
        !expected.is_empty(),
        "case now: the service publishes a catalog of updates"
    );
    let mut release_dates = BTreeSet::new();
    for (case, anchor) in anchors {
        let catalog = state::published_service_updates(anchor);
        let names: BTreeSet<String> = catalog.keys().cloned().collect();
        assert_eq!(
            names, expected,
            "case {case}: a catalog published at any instant carries the same names"
        );
        for record in catalog.values() {
            release_dates.insert(
                record
                    .release_date
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .expect("a release date after the epoch")
                    .as_secs(),
            );
        }
    }
    assert!(
        release_dates.len() >= anchors.len(),
        "case dates: the catalog's dates must hang off the anchor the store came up on, got \
         {release_dates:?}"
    );

    // Over the wire: a store whose catalog was built at an older anchor reports
    // the same names, and every one of them is still resolvable by name.
    let emulator = Emulator::new();
    let served = emulator.call("DescribeServiceUpdates", &[]).await;
    assert_eq!(
        served.status, 200,
        "case served: DescribeServiceUpdates must answer 200, got {} {}",
        served.status, served.body
    );
    let served_names: BTreeSet<String> = served
        .all("ServiceUpdateName")
        .into_iter()
        .map(str::to_string)
        .collect();
    assert_eq!(
        served_names, expected,
        "case served: the describe must report the published names, got {served_names:?}"
    );

    emulator.with_state(|store| {
        store.service_updates = state::published_service_updates(now - 4 * year);
    });
    let aged = emulator.call("DescribeServiceUpdates", &[]).await;
    assert_eq!(
        aged.status, 200,
        "case aged: DescribeServiceUpdates must answer 200, got {} {}",
        aged.status, aged.body
    );
    let aged_names: BTreeSet<String> = aged
        .all("ServiceUpdateName")
        .into_iter()
        .map(str::to_string)
        .collect();
    assert_eq!(
        aged_names, served_names,
        "case aged: an older catalog publishes the same names, got {aged_names:?}"
    );
    for name in &aged_names {
        let looked_up = emulator
            .call(
                "DescribeServiceUpdates",
                &params(&[("ServiceUpdateName", name)]),
            )
            .await;
        assert_eq!(
            looked_up.status, 200,
            "case {name}: a published name must resolve, got {} {}",
            looked_up.status, looked_up.body
        );
        assert_eq!(
            looked_up.all("ServiceUpdateName"),
            vec![name.as_str()],
            "case {name}: a name lookup reports that update alone, got {}",
            looked_up.body
        );
    }
}
