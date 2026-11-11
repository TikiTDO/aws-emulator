// SPDX-License-Identifier: CC0-1.0

//! `UpdateBrokerStorage`, driven end to end over the wire through the same
//! `MultiServiceRouter` the emulator serves from.
//!
//! The RULE under test: a broker-storage update on an ACTIVE cluster is an
//! asynchronous cluster operation. The call is checked against the cluster's
//! recorded state (its ARN must resolve in the endpoint's region, its version must
//! be the one the caller quotes, and the request must name broker `ALL` with a
//! volume size inside the EBS range that is not smaller than the one the cluster
//! already runs with), it answers the cluster's ARN plus a NEW
//! `ClusterOperationArn`, and the target storage becomes the cluster's stored
//! configuration — under a new `CurrentVersion` — when that operation's completion
//! comes due, not when the call returns.
//!
//! Each suite asserts that rule over a TABLE whose rows differ along whatever it
//! generalises over: the volume size (including both published bounds of
//! `EBSStorageInfo$VolumeSize` and one past each), whether the cluster already
//! records a size, whether provisioned throughput is part of the update, the broker
//! count the single `ALL` entry stands for, the number of entries in the target
//! list, the broker id those entries name, and the cluster's own state and version.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_kafka_kafka::state::{
    CLUSTER_ACTIVATION_DELAY, CLUSTER_UPDATE_DELAY, MAX_VOLUME_SIZE_GB, MIN_VOLUME_SIZE_GB,
};

use crate::kafka_harness::{
    assert_cluster_state_refusal, pct, sleep_past_activation, sleep_past_update, Emulator, Reply,
    ACCOUNT, REGION,
};

/// The message Amazon MSK answers a request for a resource it does not hold with,
/// spelled out here so this suite pins the sentence rather than merely checking
/// that the implementation's own constant is propagated. Note the typographic
/// apostrophe (U+2019), which is what the service sends.
const NOT_FOUND_MESSAGE: &str = "The requested resource doesn’t exist.";

/// Where a cluster's effective EBS volume size is reported, in the `ClusterInfo`
/// `DescribeCluster` answers with.
const VOLUME_SIZE_POINTER: &str =
    "/clusterInfo/brokerNodeGroupInfo/storageInfo/ebsStorageInfo/volumeSize";

/// Where the same record reports provisioned throughput.
const THROUGHPUT_POINTER: &str =
    "/clusterInfo/brokerNodeGroupInfo/storageInfo/ebsStorageInfo/provisionedThroughput";

/// A `CreateCluster` body: the required members, plus an `EbsStorageInfo` volume
/// size when the row starts the cluster with one recorded.
fn create_body(name: &str, brokers: i64, volume_size: Option<i64>) -> Value {
    let mut broker_group = json!({
        "instanceType": "kafka.m5.large",
        "clientSubnets": ["subnet-a", "subnet-b", "subnet-c"],
    });
    if let Some(size) = volume_size {
        broker_group["storageInfo"] = json!({ "ebsStorageInfo": { "volumeSize": size } });
    }
    json!({
        "clusterName": name,
        "kafkaVersion": "3.6.0",
        "numberOfBrokerNodes": brokers,
        "brokerNodeGroupInfo": broker_group,
    })
}

/// One `BrokerEBSVolumeInfo` entry, with only the members the row sets.
fn target(node_id: &str, volume_size: Option<i64>, throughput: Option<(bool, i64)>) -> Value {
    let mut entry = json!({ "kafkaBrokerNodeId": node_id });
    if let Some(size) = volume_size {
        entry["volumeSizeGB"] = json!(size);
    }
    if let Some((enabled, volume_throughput)) = throughput {
        entry["provisionedThroughput"] =
            json!({ "enabled": enabled, "volumeThroughput": volume_throughput });
    }
    entry
}

/// Create a cluster and wait out its provisioning span, so it is ACTIVE and can
/// take an update. Hands back its ARN.
async fn create_active(
    emulator: &Emulator,
    name: &str,
    brokers: i64,
    volume_size: Option<i64>,
) -> String {
    let reply = emulator
        .call("POST", "/v1/clusters", Some(create_body(name, brokers, volume_size)))
        .await;
    assert_eq!(
        reply.status, 200,
        "case {name:?}: expected CreateCluster to return 200, got {} {}",
        reply.status, reply.body
    );
    let arn = reply.body["clusterArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: CreateCluster returned no clusterArn: {}", reply.body))
        .to_string();
    sleep_past_activation();
    arn
}

/// `UpdateBrokerStorage` for `arn`, signed for the suite's region.
async fn update(emulator: &Emulator, arn: &str, current_version: &str, targets: Value) -> Reply {
    emulator
        .call(
            "PUT",
            &format!("/v1/clusters/{}/nodes/storage", pct(arn)),
            Some(json!({
                "currentVersion": current_version,
                "targetBrokerEBSVolumeInfo": targets,
            })),
        )
        .await
}

/// `DescribeClusterOperation` for `arn` — the reader of the operation an update
/// triggered, which is what its `ClusterOperationArn` names.
async fn describe_operation(emulator: &Emulator, arn: &str) -> Reply {
    emulator
        .call("GET", &format!("/v1/operations/{}", pct(arn)), None)
        .await
}

/// The cluster's `CurrentVersion`, as `DescribeCluster` reports it — the token an
/// update call has to quote.
async fn current_version(emulator: &Emulator, arn: &str) -> String {
    let described = emulator.describe(arn).await;
    described
        .at("/clusterInfo/currentVersion")
        .as_str()
        .unwrap_or_else(|| panic!("DescribeCluster reported no currentVersion for {arn}: {}", described.body))
        .to_string()
}

/// One row of the table in
/// [`a_storage_update_is_applied_when_its_operation_comes_due`]: the cluster to
/// create and the update to make to it. `None` means "the request omits it", which
/// is how a row tests what an update carries over.
#[derive(Debug, Clone, Copy)]
struct ApplyCase {
    name: &'static str,
    /// How many brokers the one `ALL` entry stands for.
    brokers: i64,
    /// The EBS volume size the cluster is created with, if any.
    recorded: Option<i64>,
    /// The volume size the update asks for, if any.
    wanted: Option<i64>,
    /// The provisioned throughput the update asks for, as `(enabled, MiB/s)`.
    throughput: Option<(bool, i64)>,
}

/// RULE: a broker-storage update writes the target volume size and provisioned
/// throughput of EVERY broker when its operation comes due — and not before, so a
/// reader in between still sees the cluster `UPDATING` at its old configuration and
/// old version.
#[tokio::test]
async fn a_storage_update_is_applied_when_its_operation_comes_due() {
    let emulator = Emulator::new();
    // Rows differ along everything the rule generalises over: the recorded size
    // (absent, small, large), the target size (both published bounds of
    // `EBSStorageInfo$VolumeSize` — 1 and 16384 — a growth, and an unchanged size),
    // whether provisioned throughput is part of the update, and the broker count
    // the one `ALL` entry stands for.
    let rows: Vec<ApplyCase> = vec![
        ApplyCase { name: "grow-recorded", brokers: 3, recorded: Some(100), wanted: Some(200), throughput: None },
        ApplyCase {
            name: "no-recorded-size",
            brokers: 3,
            recorded: None,
            wanted: Some(MIN_VOLUME_SIZE_GB.into()),
            throughput: None,
        },
        ApplyCase {
            name: "to-the-maximum",
            brokers: 1,
            recorded: Some(16000),
            wanted: Some(MAX_VOLUME_SIZE_GB.into()),
            throughput: None,
        },
        ApplyCase { name: "unchanged-size", brokers: 3, recorded: Some(500), wanted: Some(500), throughput: None },
        ApplyCase {
            name: "throughput-only",
            brokers: 15,
            recorded: Some(1000),
            wanted: None,
            throughput: Some((true, 250)),
        },
        ApplyCase {
            name: "size-and-throughput",
            brokers: 3,
            recorded: Some(100),
            wanted: Some(1024),
            throughput: Some((true, 500)),
        },
        ApplyCase {
            name: "throughput-off",
            brokers: 3,
            recorded: Some(100),
            wanted: Some(101),
            throughput: Some((false, 250)),
        },
    ];

    for row in rows {
        let ApplyCase { name, brokers, recorded, wanted, throughput } = row;
        let case = format!("{row:?}");
        let arn = create_active(&emulator, name, brokers, recorded).await;
        let before = emulator.describe(&arn).await;
        assert_eq!(
            before.at("/clusterInfo/state").as_str(),
            Some("ACTIVE"),
            "case {case}: the cluster must be ACTIVE before the update, got {}",
            before.body
        );
        let version_before = current_version(&emulator, &arn).await;

        let started = Instant::now();
        let updated = update(
            &emulator,
            &arn,
            &version_before,
            json!([target("ALL", wanted, throughput)]),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected UpdateBrokerStorage to return 200, got {} {}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.body["clusterArn"].as_str(),
            Some(arn.as_str()),
            "case {case}: the update must name the cluster it updated, got {}",
            updated.body
        );
        let operation_arn = updated.body["clusterOperationArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: no clusterOperationArn in {}", updated.body));
        assert!(
            operation_arn
                .starts_with(&format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster-operation/{name}/")),
            "case {case}: expected a cluster-operation ARN for this cluster, got {operation_arn:?}"
        );

        // Before the operation is due the cluster is UPDATING at its OLD
        // configuration, reporting the operation it triggered as its ACTIVE one:
        // the change is owed to the cluster, not done by the call. The answer is
        // taken first and only asserted when the span provably had not elapsed by
        // then, so a slow machine cannot make this a false failure.
        let pending = emulator.describe(&arn).await;
        if started.elapsed() < CLUSTER_UPDATE_DELAY {
            assert_eq!(
                pending.at("/clusterInfo/state").as_str(),
                Some("UPDATING"),
                "case {case}: a describe straight after the update must see UPDATING, got {}",
                pending.body
            );
            assert_eq!(
                pending.at("/clusterInfo/activeOperationArn").as_str(),
                Some(operation_arn),
                "case {case}: the cluster must report the triggered operation as its active one, got {}",
                pending.body
            );
            assert_eq!(
                pending.at(VOLUME_SIZE_POINTER).as_i64(),
                recorded,
                "case {case}: the volume size must not change before the operation is due, got {}",
                pending.body
            );
            assert_eq!(
                pending.at("/clusterInfo/currentVersion").as_str(),
                Some(version_before.as_str()),
                "case {case}: the version must not change before the operation is due, got {}",
                pending.body
            );
        }

        sleep_past_update();
        let after = emulator.describe(&arn).await;
        assert_eq!(
            after.at("/clusterInfo/state").as_str(),
            Some("ACTIVE"),
            "case {case}: the cluster must be ACTIVE once the operation is due, got {}",
            after.body
        );
        // What the update did not mention is carried over, which is why the
        // expected size falls back to the recorded one.
        let expected_size = wanted.or(recorded);
        assert_eq!(
            after.at(VOLUME_SIZE_POINTER).as_i64(),
            expected_size,
            "case {case}: expected volumeSize {expected_size:?} after the operation, got {}",
            after.body
        );
        let expected_throughput = throughput.map_or(Value::Null, |(enabled, volume_throughput)| {
            json!({ "enabled": enabled, "volumeThroughput": volume_throughput })
        });
        assert_eq!(
            after.at(THROUGHPUT_POINTER),
            &expected_throughput,
            "case {case}: expected provisionedThroughput {expected_throughput} after the operation, got {}",
            after.body
        );
        let version_after = after
            .at("/clusterInfo/currentVersion")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_ne!(
            version_after, version_before,
            "case {case}: an applied update must generate a new version, got {version_after:?}"
        );

        // Reading it again moves nothing on: the clock owed one completion.
        let again = emulator.describe(&arn).await;
        assert_eq!(
            (
                again.at("/clusterInfo/state").as_str(),
                again.at("/clusterInfo/currentVersion").as_str(),
                again.at(VOLUME_SIZE_POINTER).as_i64()
            ),
            (
                Some("ACTIVE"),
                Some(version_after.as_str()),
                expected_size
            ),
            "case {case}: a second read must report the same settled cluster, got {}",
            again.body
        );

        // And the finished cluster reports no operation in flight any more.
        assert!(
            again.at("/clusterInfo/activeOperationArn").is_null(),
            "case {case}: a settled cluster must report no active operation, got {}",
            again.body
        );

        // The operation itself is readable, and says what it changed: this
        // operation's own type, the cluster it ran on, the storage it started from
        // and the storage it produced.
        let operation = describe_operation(&emulator, operation_arn).await;
        assert_eq!(
            (
                operation.status,
                operation.at("/clusterOperationInfo/operationType").as_str(),
                operation.at("/clusterOperationInfo/clusterArn").as_str(),
                operation.at("/clusterOperationInfo/operationState").as_str(),
            ),
            (
                200,
                Some("UPDATE_BROKER_STORAGE"),
                Some(arn.as_str()),
                Some("UPDATE_COMPLETE")
            ),
            "case {case}: expected a finished broker-storage operation, got {} {}",
            operation.status,
            operation.body
        );
        assert_eq!(
            (
                operation
                    .at("/clusterOperationInfo/sourceClusterInfo/brokerEBSVolumeInfo/0/volumeSizeGB")
                    .as_i64(),
                operation
                    .at("/clusterOperationInfo/targetClusterInfo/brokerEBSVolumeInfo/0/volumeSizeGB")
                    .as_i64(),
            ),
            (recorded, expected_size),
            "case {case}: the operation must report the sizes it moved between, got {}",
            operation.body
        );
    }
}

/// RULE: every update generates the next version of the cluster and its own
/// operation, so versions are never reused, an operation ARN is never reused, and
/// the version an earlier update was made from is no longer accepted.
#[tokio::test]
async fn each_update_generates_a_new_version_and_its_own_operation() {
    let emulator = Emulator::new();
    let arn = create_active(&emulator, "versioned", 3, Some(100)).await;

    let mut versions = vec![current_version(&emulator, &arn).await];
    let mut operations: Vec<String> = Vec::new();
    // Rows: a growing sequence of volume sizes, each update made from the version
    // the previous one generated.
    for size in [200_i64, 400, 1600] {
        let from = versions.last().expect("the first version was recorded").clone();
        let updated = update(&emulator, &arn, &from, json!([target("ALL", Some(size), None)])).await;
        assert_eq!(
            updated.status, 200,
            "case size={size}: expected UpdateBrokerStorage to return 200, got {} {}",
            updated.status, updated.body
        );
        operations.push(
            updated.body["clusterOperationArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case size={size}: no clusterOperationArn in {}", updated.body))
                .to_string(),
        );
        sleep_past_update();

        let described = emulator.describe(&arn).await;
        assert_eq!(
            described.at(VOLUME_SIZE_POINTER).as_i64(),
            Some(size),
            "case size={size}: expected the new volume size to be stored, got {}",
            described.body
        );
        let version = described
            .at("/clusterInfo/currentVersion")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !versions.contains(&version),
            "case size={size}: expected a version not seen before, got {version:?} after {versions:?}"
        );

        // The version this update was made from is spent: quoting it again is a
        // stale-version refusal, not a second update.
        let stale = update(&emulator, &arn, &from, json!([target("ALL", Some(size + 1), None)])).await;
        assert_eq!(
            (stale.status, stale.error_code()),
            (400, "BadRequestException"),
            "case size={size}: expected a 400 BadRequestException for the spent version {from:?}, got {} {}",
            stale.status,
            stale.body
        );
        assert_eq!(
            emulator.describe(&arn).await.at(VOLUME_SIZE_POINTER).as_i64(),
            Some(size),
            "case size={size}: a refused update must leave the cluster alone"
        );

        versions.push(version);
    }

    let mut distinct = operations.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        operations.len(),
        "each update must mint its own operation ARN, got {operations:?}"
    );
}

/// RULE: an update the cluster cannot make is refused with the modeled client
/// error — `NotFoundException` (404) for a well-formed `ClusterArn` this endpoint's
/// region holds no cluster for, `BadRequestException` (400) for everything the
/// request itself gets wrong — and the cluster is left exactly as it was.
#[tokio::test]
async fn an_update_outside_the_rule_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let arn = create_active(&emulator, "refusals", 3, Some(100)).await;
    let version = current_version(&emulator, &arn).await;
    let absent = format!(
        "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/absent/11111111-2222-4333-8444-555555555555-1"
    );
    let wrong_resource_type = format!("arn:aws:kafka:{REGION}:{ACCOUNT}:configuration/refusals/abc");
    let another_region = format!(
        "arn:aws:kafka:us-east-1:{ACCOUNT}:cluster/refusals/11111111-2222-4333-8444-555555555555-1"
    );

    // `(case, cluster arn, currentVersion, targetBrokerEBSVolumeInfo, status, code)`.
    // Rows: each way the ARN can fail, each way the target list can fail (count,
    // broker id, nothing to set), each bound of `EBSStorageInfo$VolumeSize` passed
    // by one, a shrink, and a stale version.
    let rows: Vec<(&str, &str, &str, Value, u16, &str)> = vec![
        (
            "malformed-arn",
            "not-an-arn",
            version.as_str(),
            json!([target("ALL", Some(200), None)]),
            400,
            "BadRequestException",
        ),
        (
            "wrong-resource-type",
            wrong_resource_type.as_str(),
            version.as_str(),
            json!([target("ALL", Some(200), None)]),
            400,
            "BadRequestException",
        ),
        (
            "absent-cluster",
            absent.as_str(),
            version.as_str(),
            json!([target("ALL", Some(200), None)]),
            404,
            "NotFoundException",
        ),
        (
            "another-regions-arn",
            another_region.as_str(),
            version.as_str(),
            json!([target("ALL", Some(200), None)]),
            404,
            "NotFoundException",
        ),
        (
            "no-target-entries",
            arn.as_str(),
            version.as_str(),
            json!([]),
            400,
            "BadRequestException",
        ),
        (
            "two-target-entries",
            arn.as_str(),
            version.as_str(),
            json!([target("ALL", Some(200), None), target("ALL", Some(300), None)]),
            400,
            "BadRequestException",
        ),
        (
            "numbered-broker",
            arn.as_str(),
            version.as_str(),
            json!([target("1", Some(200), None)]),
            400,
            "BadRequestException",
        ),
        (
            "lower-case-all",
            arn.as_str(),
            version.as_str(),
            json!([target("all", Some(200), None)]),
            400,
            "BadRequestException",
        ),
        (
            "empty-broker-id",
            arn.as_str(),
            version.as_str(),
            json!([target("", Some(200), None)]),
            400,
            "BadRequestException",
        ),
        (
            "nothing-to-set",
            arn.as_str(),
            version.as_str(),
            json!([target("ALL", None, None)]),
            400,
            "BadRequestException",
        ),
        (
            "one-below-the-minimum",
            arn.as_str(),
            version.as_str(),
            json!([target("ALL", Some(i64::from(MIN_VOLUME_SIZE_GB) - 1), None)]),
            400,
            "BadRequestException",
        ),
        (
            "one-above-the-maximum",
            arn.as_str(),
            version.as_str(),
            json!([target("ALL", Some(i64::from(MAX_VOLUME_SIZE_GB) + 1), None)]),
            400,
            "BadRequestException",
        ),
        (
            "negative-size",
            arn.as_str(),
            version.as_str(),
            json!([target("ALL", Some(-5), None)]),
            400,
            "BadRequestException",
        ),
        (
            "shrink",
            arn.as_str(),
            version.as_str(),
            json!([target("ALL", Some(50), None)]),
            400,
            "BadRequestException",
        ),
        (
            "stale-version",
            arn.as_str(),
            "K0000000000000",
            json!([target("ALL", Some(200), None)]),
            400,
            "BadRequestException",
        ),
    ];

    for (case, cluster_arn, quoted_version, targets, status, code) in rows {
        let reply = update(&emulator, cluster_arn, quoted_version, targets).await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (status, code),
            "case {case:?}: expected {status} {code}, got {} {}",
            reply.status,
            reply.body
        );
        if code == "NotFoundException" {
            // The service answers a resource it does not hold with its own
            // constant sentence, whichever operation was called.
            assert_eq!(
                reply.body["message"].as_str(),
                Some(NOT_FOUND_MESSAGE),
                "case {case:?}: expected the service's not-found message, got {}",
                reply.body
            );
        }
        let described = emulator.describe(&arn).await;
        assert_eq!(
            (
                described.at("/clusterInfo/state").as_str(),
                described.at("/clusterInfo/currentVersion").as_str(),
                described.at(VOLUME_SIZE_POINTER).as_i64()
            ),
            (Some("ACTIVE"), Some(version.as_str()), Some(100)),
            "case {case:?}: a refused update must leave the cluster exactly as it was, got {}",
            described.body
        );
    }

    // The member omitted entirely is refused too, by whichever gate sees it first:
    // the boundary validator (`ValidationException`) or the handler
    // (`BadRequestException`).
    let omitted = emulator
        .call(
            "PUT",
            &format!("/v1/clusters/{}/nodes/storage", pct(&arn)),
            Some(json!({ "currentVersion": version })),
        )
        .await;
    assert_eq!(
        omitted.status, 400,
        "case \"omitted-target\": expected a 400, got {} {}",
        omitted.status, omitted.body
    );
    assert!(
        matches!(
            omitted.error_code(),
            "BadRequestException" | "ValidationException"
        ),
        "case \"omitted-target\": expected a client-input refusal, got {}",
        omitted.body
    );
}

/// RULE: only a cluster with a settled configuration can be updated — one still
/// provisioning, and one already rolling a change across its brokers, are refused —
/// and a refusal neither starts an operation nor disturbs the one in flight.
#[tokio::test]
async fn a_cluster_that_is_not_active_takes_no_storage_update() {
    let emulator = Emulator::new();
    // Rows: a cluster with a recorded size and one without, so the state gate does
    // not depend on the stored configuration.
    for (name, recorded) in [("still-creating", Some(100_i64)), ("creating-bare", None)] {
        let created = emulator
            .call("POST", "/v1/clusters", Some(create_body(name, 3, recorded)))
            .await;
        let started = Instant::now();
        let arn = created.body["clusterArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: no clusterArn in {}", created.body))
            .to_string();
        let version = current_version(&emulator, &arn).await;
        let reply = update(&emulator, &arn, &version, json!([target("ALL", Some(200), None)])).await;
        let within_the_creating_span = started.elapsed() < CLUSTER_ACTIVATION_DELAY;
        if within_the_creating_span {
            assert_eq!(
                (reply.status, reply.error_code()),
                (400, "BadRequestException"),
                "case {name:?}: expected a 400 BadRequestException while the cluster is CREATING, got {} {}",
                reply.status,
                reply.body
            );
        } else {
            // The provisioning span may have elapsed on a loaded machine, in which
            // case the cluster was ACTIVE by the time the update ran and the update
            // is the accepted one — never a success against a CREATING cluster, and
            // never any other error.
            assert!(
                reply.status == 200 || (reply.status, reply.error_code()) == (400, "BadRequestException"),
                "case {name:?}: expected the state refusal or an accepted update, got {} {}",
                reply.status,
                reply.body
            );
        }
        // The cluster finishes provisioning either way, and only an ACCEPTED update
        // may have changed its storage.
        sleep_past_activation();
        sleep_past_update();
        let described = emulator.describe(&arn).await;
        let expected_size = if reply.status == 200 { Some(200) } else { recorded };
        assert_eq!(
            (
                described.at("/clusterInfo/state").as_str(),
                described.at(VOLUME_SIZE_POINTER).as_i64()
            ),
            (Some("ACTIVE"), expected_size),
            "case {name:?}: a refused update must not change the provisioning cluster, got {}",
            described.body
        );
    }

    // And a second update while the first is still in flight.
    let arn = create_active(&emulator, "already-updating", 3, Some(100)).await;
    let version = current_version(&emulator, &arn).await;
    let first = update(&emulator, &arn, &version, json!([target("ALL", Some(200), None)])).await;
    let started = Instant::now();
    assert_eq!(
        first.status, 200,
        "expected the first update to be accepted, got {} {}",
        first.status, first.body
    );
    let second = update(&emulator, &arn, &version, json!([target("ALL", Some(300), None)])).await;
    // Refused either way, and for a reason that is this rule: while the first
    // operation is pending the cluster is UPDATING, and once it has completed the
    // cluster is at a NEW version, so the version this call quotes is spent. Never
    // a success — two updates from one version are never both accepted.
    assert_eq!(
        (second.status, second.error_code()),
        (400, "BadRequestException"),
        "expected a 400 BadRequestException for a second update from the same version, got {} {}",
        second.status,
        second.body
    );
    if started.elapsed() < CLUSTER_UPDATE_DELAY {
        // While the first operation is still in flight the cluster is UPDATING, so
        // the refusal is the state one: the service words it in the one sentence
        // that names the states a change IS accepted from.
        assert_cluster_state_refusal(&second, "second update while the first is in flight");
    }

    // The update that WAS accepted still lands, unaffected by the refused one.
    sleep_past_update();
    let described = emulator.describe(&arn).await;
    assert_eq!(
        (
            described.at("/clusterInfo/state").as_str(),
            described.at(VOLUME_SIZE_POINTER).as_i64()
        ),
        (Some("ACTIVE"), Some(200)),
        "the accepted update must still be the one applied, got {}",
        described.body
    );
}

/// RULE: the change an update records is owed to its cluster at a stored INSTANT —
/// the cluster reports the pre-update configuration, the `UPDATING` state and the
/// operation as its active one until then, and the new configuration plus the next
/// version from then on — and it is the clock that decides, not the reader.
///
/// Driven against the state record rather than over the wire, so `now` is a value
/// this test chooses: both halves of the rule are asserted exactly, with no
/// dependence on how fast the machine is.
#[tokio::test]
async fn an_owed_change_lands_exactly_at_its_due_instant() {
    use std::time::SystemTime;

    use service_com_amazonaws_kafka_kafka::ops::types::{
        BrokerNodeGroupInfo, CreateClusterInput, EBSStorageInfo, ProvisionedThroughput,
        StorageInfo,
    };
    use service_com_amazonaws_kafka_kafka::state::{
        service_state, Cluster, StorageChange, CLUSTER_STATE_ACTIVE, CLUSTER_STATE_UPDATING,
        CLUSTER_UPDATE_DELAY, OPERATION_TYPE_UPDATE_BROKER_STORAGE,
    };

    // The shared store a handler reaches through `self.engine`; never constructed
    // inside a handler, which is what `state_roundtrip.rs` pins.
    let engine = engine::InMemoryEngine::new();
    let store = service_state(&engine).expect("the kafka service state is reachable");
    let created = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let cluster = Cluster::provisioned(
        REGION,
        0,
        created,
        CreateClusterInput {
            cluster_name: "clocked".to_string(),
            kafka_version: "3.6.0".to_string(),
            number_of_broker_nodes: 3,
            broker_node_group_info: BrokerNodeGroupInfo {
                instance_type: "kafka.m5.large".to_string(),
                client_subnets: vec!["subnet-a".to_string()],
                storage_info: Some(StorageInfo {
                    ebs_storage_info: Some(EBSStorageInfo {
                        provisioned_throughput: None,
                        volume_size: Some(100),
                    }),
                }),
                ..BrokerNodeGroupInfo::default()
            },
            ..CreateClusterInput::default()
        },
    );
    let cluster_arn = cluster.arn.clone();
    let mut state = store.lock();
    state
        .register_cluster(cluster)
        .expect("a fresh registry accepts the first cluster");
    // Settled by the clock, not by a call: the cluster is ACTIVE once its
    // provisioning instant has passed.
    let active_at = created + Duration::from_secs(1);
    let version_before = state
        .settled_cluster_in(REGION, &cluster_arn, active_at)
        .map(|cluster| (cluster.state.clone(), cluster.current_version.clone()))
        .expect("the cluster is registered");
    assert_eq!(
        version_before.0, CLUSTER_STATE_ACTIVE,
        "the cluster must be ACTIVE once its provisioning instant has passed"
    );

    // The same path the handler starts an update through, so this test exercises
    // the registry's own write rather than a test-only shortcut.
    let operation_arn = state
        .start_storage_update(
            REGION,
            &cluster_arn,
            active_at,
            OPERATION_TYPE_UPDATE_BROKER_STORAGE,
            StorageChange {
                volume_size_gb: Some(400),
                provisioned_throughput: Some(ProvisionedThroughput {
                    enabled: Some(true),
                    volume_throughput: Some(250),
                }),
                storage_mode: None,
            },
        )
        .expect("the registered cluster takes an update");
    let due = active_at + CLUSTER_UPDATE_DELAY;

    // Rows: the instants that matter — the call's own instant, a tick before the
    // change is due, the due instant itself, and long after. The rule generalises
    // over the clock, so the clock is what the rows differ along.
    for before in [active_at, due - Duration::from_millis(1)] {
        let cluster = state
            .settled_cluster_in(REGION, &cluster_arn, before)
            .expect("the cluster is registered");
        assert_eq!(
            (
                cluster.state.as_str(),
                cluster.volume_size_gb(),
                cluster.current_version.as_str(),
                cluster.to_cluster_info().active_operation_arn.as_deref()
            ),
            (
                CLUSTER_STATE_UPDATING,
                Some(100),
                version_before.1.as_str(),
                Some(operation_arn.as_str())
            ),
            "case now={before:?}: nothing the readers report may change before the change is due"
        );
    }
    for at_or_after in [due, due + Duration::from_secs(3_600)] {
        let cluster = state
            .settled_cluster_in(REGION, &cluster_arn, at_or_after)
            .expect("the cluster is registered");
        assert_eq!(
            (
                cluster.state.as_str(),
                cluster.volume_size_gb(),
                cluster
                    .provisioned_throughput()
                    .and_then(|throughput| throughput.volume_throughput),
                cluster.to_cluster_info().active_operation_arn
            ),
            (CLUSTER_STATE_ACTIVE, Some(400), Some(250), None),
            "case now={at_or_after:?}: the change must have landed by its due instant"
        );
        assert_ne!(
            cluster.current_version, version_before.1,
            "case now={at_or_after:?}: an applied update must generate a new version"
        );
    }
}
