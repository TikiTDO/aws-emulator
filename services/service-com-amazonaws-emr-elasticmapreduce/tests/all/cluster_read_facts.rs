// SPDX-License-Identifier: CC0-1.0

//! What a read of a cluster DERIVES rather than echoes, end to end over the wire.
//!
//! Six rules, each about a member `DescribeCluster` answers for a cluster whose launch did
//! not state it:
//!
//! * `NormalizedInstanceHours` is "an approximation of the cost of the cluster, represented
//!   in m1.small/hours. This value is incremented one time for every hour an m1.small
//!   instance runs. Larger instances are weighted more", so it is the weight of the
//!   instances the cluster was created with, times the hours it has been up — a cluster that
//!   has run accrues it, and a read never changes it;
//! * `OSReleaseLabel` is "the Amazon Linux release specified in a cluster launch RunJobFlow
//!   request. If no Amazon Linux release was specified, the default Amazon Linux release is
//!   shown in the response", so a described cluster always states the Amazon Linux release
//!   it runs;
//! * `Ec2InstanceAttributes.Ec2AvailabilityZone` is "the Availability Zone in which the
//!   cluster will run", and a launch that placed the cluster in a SUBNET named that zone
//!   through the subnet — which Amazon EC2 owns, so the zone is the one EC2 reports for it;
//! * `MasterPublicDnsName` is "the DNS name of the master node", an address a client
//!   CONNECTS to, so a cluster of an emulator that answers on a host of its own is named by
//!   that host rather than by one nothing resolves;
//! * `UnhealthyNodeReplacement` "indicates whether Amazon EMR should gracefully replace
//!   Amazon EC2 core instances that have degraded within the cluster", which a cluster with
//!   more than one primary node is not offered at all and every other cluster runs with the
//!   launch's own value or the service's default;
//! * an `Applications` entry's `Version` is the version the cluster's `ReleaseLabel` ships,
//!   read off the one release catalog `DescribeReleaseLabel` publishes.
//!
//! Amazon EC2 is mounted BESIDE Amazon EMR on one engine and every subnet a row names was
//! MINTED by EC2 itself, so the zone a cluster reports is the zone the owning service holds.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_emr_elasticmapreduce::state::CLUSTER_PHASE_SPAN;
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

/// The Region every request below is signed for.
const REGION: &str = "us-west-2";

/// The roles a launch names.
const INSTANCE_PROFILE: &str = "EMR_EC2_DefaultRole";
const SERVICE_ROLE: &str = "EMR_DefaultRole";

/// How long a read waits for a launched cluster to have become ready: both provisioning
/// phases and a margin for the harness itself.
const PAST_CLUSTER_READY: Duration = Duration::from_millis(
    (CLUSTER_PHASE_SPAN.as_millis() as u64) * 2 + 500,
);

/// Amazon EMR and Amazon EC2 on one engine, plus an EC2 client over the router's own
/// in-process callback — the seam this service resolves a subnet through.
struct Emulator {
    router: MultiServiceRouter,
    ec2: ::aws_sdk_ec2::Client,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_emr_elasticmapreduce::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    let callback = EmbeddedEmulator::from_router(router.clone());
    let config = ::aws_sdk_ec2::Config::builder()
        .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
        .region(::aws_sdk_ec2::config::Region::new(REGION.to_string()))
        .endpoint_url(format!("https://ec2.{REGION}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled())
        .build();
    Emulator {
        router,
        ec2: ::aws_sdk_ec2::Client::from_conf(config),
    }
}

/// One `awsJson1_1` Amazon EMR call addressed to `host`, which must be served.
async fn emr_at(router: &MultiServiceRouter, host: &str, operation: &str, body: Value) -> Value {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", host)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("ElasticMapReduce.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20261109/{REGION}/\
                 elasticmapreduce/aws4_request, SignedHeaders=host, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "{operation} must answer 200, got {status} with {payload}"
    );
    payload
}

/// One Amazon EMR call addressed to this service's own published host.
async fn emr(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let host = format!("elasticmapreduce.{REGION}.amazonaws.com");
    emr_at(router, &host, operation, body).await
}

/// A cluster launched with `members` merged into the request, answering its id.
async fn launch(router: &MultiServiceRouter, name: &str, members: Value) -> String {
    let mut request = json!({
        "Name": name,
        "JobFlowRole": INSTANCE_PROFILE,
        "ServiceRole": SERVICE_ROLE,
        "Instances": {
            "InstanceCount": 1,
            "MasterInstanceType": "m5.xlarge",
            "KeepJobFlowAliveWhenNoSteps": true,
        },
    });
    for (member, value) in members.as_object().cloned().unwrap_or_default() {
        request[member] = value;
    }
    emr(router, "RunJobFlow", request).await["JobFlowId"]
        .as_str()
        .expect("RunJobFlow answers a JobFlowId")
        .to_string()
}

/// The cluster `DescribeCluster` reports.
async fn described(router: &MultiServiceRouter, cluster_id: &str) -> Value {
    emr(router, "DescribeCluster", json!({ "ClusterId": cluster_id })).await["Cluster"].clone()
}

/// THE RULE for the accrual: the hours a cluster has been up, times the weight of the
/// instances it was created with.
///
/// The rows span what the rule generalises over: the uniform launch form and the instance
/// group and instance fleet forms, one instance and several, and four instance SIZES — so
/// the weights are read off the catalog rather than off one row. Every cluster here is
/// seconds old, which is part of the first instance hour, so the expected value is the
/// weight of its instances.
#[tokio::test]
async fn a_cluster_accrues_the_normalized_hours_its_instances_weigh() {
    // CATALOG DATA: the normalization factor of each size, from the Amazon EMR Management
    // Guide's normalized-instance-hours table — xlarge weighs 8 m1.small hours per hour,
    // large 4, 2xlarge 16 and 4xlarge 32.
    let cases: Vec<(&str, Value, i64)> = vec![
        (
            "one xlarge, the uniform form",
            json!({ "InstanceCount": 1, "MasterInstanceType": "m5.xlarge" }),
            8,
        ),
        (
            "a primary and a core node, the uniform form",
            json!({
                "InstanceCount": 2,
                "MasterInstanceType": "m5.xlarge",
                "SlaveInstanceType": "m5.xlarge",
            }),
            16,
        ),
        (
            "a large primary, the uniform form",
            json!({ "InstanceCount": 1, "MasterInstanceType": "m5.large" }),
            4,
        ),
        (
            "instance groups of two sizes",
            json!({
                "InstanceGroups": [
                    { "InstanceRole": "MASTER", "InstanceType": "m5.4xlarge", "InstanceCount": 1 },
                    { "InstanceRole": "CORE", "InstanceType": "m5.xlarge", "InstanceCount": 3 },
                ],
            }),
            32 + 24,
        ),
        (
            "an instance fleet whose capacity is three instances",
            json!({
                "InstanceFleets": [{
                    "InstanceFleetType": "MASTER",
                    "TargetOnDemandCapacity": 3,
                    "InstanceTypeConfigs": [{ "InstanceType": "m5.2xlarge" }],
                }],
            }),
            48,
        ),
    ];

    let emulator = emulator();
    for (label, instances, expected) in &cases {
        let mut members = json!({ "ReleaseLabel": "emr-7.9.0" });
        let mut request = instances.clone();
        request["KeepJobFlowAliveWhenNoSteps"] = json!(true);
        members["Instances"] = request;
        let cluster_id = launch(&emulator.router, label, members).await;
        let cluster = described(&emulator.router, &cluster_id).await;
        let accrued = cluster["NormalizedInstanceHours"].as_i64();
        assert_eq!(
            accrued,
            Some(*expected),
            "case {label:?}: expected {expected} normalized instance hours, got {cluster}"
        );

        // A read changes nothing: the same cluster accrues the same hours again, and the
        // listing reports the same number as the detail.
        let again = described(&emulator.router, &cluster_id).await;
        assert_eq!(
            again["NormalizedInstanceHours"].as_i64(),
            accrued,
            "case {label:?}: a second read must report the same accrual, got {again}"
        );
        let listed = emr(&emulator.router, "ListClusters", json!({})).await;
        let summary = listed["Clusters"]
            .as_array()
            .and_then(|clusters| {
                clusters
                    .iter()
                    .find(|cluster| cluster["Id"].as_str() == Some(cluster_id.as_str()))
                    .cloned()
            })
            .unwrap_or_else(|| panic!("case {label:?}: ListClusters must report {cluster_id}"));
        assert_eq!(
            summary["NormalizedInstanceHours"].as_i64(),
            accrued,
            "case {label:?}: the listing reports the same accrual, got {summary}"
        );
    }
}

/// THE RULE for the Amazon Linux release: the launch's own value, or the default release of
/// the generation the cluster's Amazon EMR release is built on.
///
/// The rows span what the rule generalises over: a launch that named a release, one on each
/// Amazon EMR generation that named none (Amazon Linux 2 for the 5.x and 6.x series, Amazon
/// Linux 2023 for 7.x), and a launch that named no Amazon EMR release at all — which names
/// no generation, so no Amazon Linux release is stated for it.
#[tokio::test]
async fn a_cluster_states_the_amazon_linux_release_it_runs() {
    // The release-note form `OSRelease$Label` documents: "as shown in Amazon Linux 2 Release
    // Notes. For example, 2.0.20220218.1."
    let cases: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "the release the launch named",
            json!({ "ReleaseLabel": "emr-7.9.0", "OSReleaseLabel": "2023.4.20240429.0" }),
            Some("2023.4.20240429.0"),
        ),
        (
            "a 7.x launch that named none",
            json!({ "ReleaseLabel": "emr-7.9.0" }),
            None,
        ),
        (
            "a 6.x launch that named none",
            json!({ "ReleaseLabel": "emr-6.15.0" }),
            None,
        ),
        (
            "a 5.x launch that named none",
            json!({ "ReleaseLabel": "emr-5.36.1" }),
            None,
        ),
    ];
    // The generation each Amazon EMR series is built on, which the default has to belong to.
    let generation = |release: &str| -> &'static str {
        if release.starts_with("emr-7") {
            "2023."
        } else {
            "2.0."
        }
    };

    let emulator = emulator();
    for (label, members, exact) in &cases {
        let cluster_id = launch(&emulator.router, label, members.clone()).await;
        let cluster = described(&emulator.router, &cluster_id).await;
        let reported = cluster["OSReleaseLabel"].as_str().map(str::to_string);
        match exact {
            Some(named) => assert_eq!(
                reported.as_deref(),
                Some(*named),
                "case {label:?}: the launch's own release is echoed, got {cluster}"
            ),
            None => {
                let release = members["ReleaseLabel"].as_str().unwrap_or_default();
                let prefix = generation(release);
                let stated = reported.clone().unwrap_or_else(|| {
                    panic!("case {label:?}: a described cluster states a release, got {cluster}")
                });
                assert!(
                    stated.starts_with(prefix),
                    "case {label:?}: expected a {prefix}x release for {release}, got {stated:?}"
                );
            }
        }
    }

    // A launch that named no Amazon EMR release names no generation to default within.
    let cluster_id = launch(&emulator.router, "no release label", json!({})).await;
    let cluster = described(&emulator.router, &cluster_id).await;
    assert!(
        cluster["OSReleaseLabel"].is_null(),
        "a cluster on no named release states no Amazon Linux release, got {cluster}"
    );
}

/// THE RULE for the zone: the zone of the subnet the launch placed the cluster in, as the
/// service that owns subnets reports it.
///
/// The rows span what the rule generalises over: a subnet Amazon EC2 really holds (in each
/// of two zones, so the answer is the subnet's own rather than the Region's first), a launch
/// that named the zone outright, a launch that named a subnet Amazon EC2 holds nothing
/// under, and a launch that named no placement at all.
#[tokio::test]
async fn a_cluster_runs_in_the_zone_of_the_subnet_it_was_placed_in() {
    let emulator = emulator();
    let vpc = emulator
        .ec2
        .create_vpc()
        .cidr_block("10.0.0.0/16")
        .send()
        .await
        .expect("EC2 must create the VPC");
    let vpc_id = vpc
        .vpc()
        .and_then(::aws_sdk_ec2::types::Vpc::vpc_id)
        .expect("CreateVpc reports the VPC it minted")
        .to_string();

    // One subnet per zone, each in the zone EC2 was asked for, and each id the one EC2
    // minted.
    let mut placed: Vec<(String, String)> = Vec::new();
    for (zone, cidr) in [("us-west-2a", "10.0.1.0/24"), ("us-west-2c", "10.0.2.0/24")] {
        let created = emulator
            .ec2
            .create_subnet()
            .vpc_id(&vpc_id)
            .cidr_block(cidr)
            .availability_zone(zone)
            .send()
            .await
            .unwrap_or_else(|error| panic!("EC2 must create the subnet in {zone}: {error:?}"));
        let subnet = created
            .subnet()
            .expect("CreateSubnet reports the subnet it minted");
        placed.push((
            subnet
                .subnet_id()
                .expect("a minted subnet carries an id")
                .to_string(),
            subnet
                .availability_zone()
                .unwrap_or(zone)
                .to_string(),
        ));
    }

    for (subnet_id, zone) in &placed {
        let label = format!("a cluster in {subnet_id}");
        let cluster_id = launch(
            &emulator.router,
            &label,
            json!({
                "ReleaseLabel": "emr-7.9.0",
                "Instances": {
                    "InstanceCount": 1,
                    "MasterInstanceType": "m5.xlarge",
                    "KeepJobFlowAliveWhenNoSteps": true,
                    "Ec2SubnetId": subnet_id,
                },
            }),
        )
        .await;
        let cluster = described(&emulator.router, &cluster_id).await;
        let attributes = &cluster["Ec2InstanceAttributes"];
        assert_eq!(
            attributes["Ec2AvailabilityZone"].as_str(),
            Some(zone.as_str()),
            "case {label:?}: expected the subnet's own zone {zone:?}, got {cluster}"
        );
        assert_eq!(
            attributes["Ec2SubnetId"].as_str(),
            Some(subnet_id.as_str()),
            "case {label:?}: the cluster reports the subnet it runs in, got {cluster}"
        );
        // The candidate lists are collections the cluster carries: this launch named one
        // subnet outright and no candidates, so both are reported empty rather than absent.
        for member in ["RequestedEc2SubnetIds", "RequestedEc2AvailabilityZones"] {
            assert_eq!(
                attributes[member].as_array(),
                Some(&Vec::new()),
                "case {label:?}: {member} must be reported as an empty list, got {cluster}"
            );
        }
    }

    // A zone named outright is the zone, with no subnet to resolve.
    let named = launch(
        &emulator.router,
        "a zone named outright",
        json!({
            "ReleaseLabel": "emr-7.9.0",
            "Instances": {
                "InstanceCount": 1,
                "MasterInstanceType": "m5.xlarge",
                "KeepJobFlowAliveWhenNoSteps": true,
                "Placement": { "AvailabilityZone": "us-west-2d" },
            },
        }),
    )
    .await;
    let cluster = described(&emulator.router, &named).await;
    assert_eq!(
        cluster["Ec2InstanceAttributes"]["Ec2AvailabilityZone"].as_str(),
        Some("us-west-2d"),
        "a launch that named its zone runs in it, got {cluster}"
    );

    // A subnet the owning service holds nothing under resolves to no zone: nothing is known
    // about where such a cluster runs, so nothing is reported.
    for (label, instances) in [
        (
            "a subnet Amazon EC2 holds nothing under",
            json!({
                "InstanceCount": 1,
                "MasterInstanceType": "m5.xlarge",
                "KeepJobFlowAliveWhenNoSteps": true,
                "Ec2SubnetId": "subnet-0000000000bad0bad",
            }),
        ),
        (
            "no placement at all",
            json!({
                "InstanceCount": 1,
                "MasterInstanceType": "m5.xlarge",
                "KeepJobFlowAliveWhenNoSteps": true,
            }),
        ),
    ] {
        let cluster_id = launch(
            &emulator.router,
            label,
            json!({ "ReleaseLabel": "emr-7.9.0", "Instances": instances }),
        )
        .await;
        let cluster = described(&emulator.router, &cluster_id).await;
        assert!(
            cluster["Ec2InstanceAttributes"]["Ec2AvailabilityZone"].is_null(),
            "case {label:?}: no zone is invented, got {cluster}"
        );
    }
}

/// THE RULE for the master node's name: a cluster whose master node is up is named by the
/// host this emulator answers on, where it answers on one.
///
/// The rows span what the rule generalises over: a client that reached the emulator's own
/// endpoint (with a port and without, by address and by name) — each of which is told that
/// host — and a client that resolves AWS's own hostnames, which is told the Amazon-provided
/// private hostname of the node's address.
#[tokio::test]
async fn a_cluster_names_its_master_node_by_the_host_this_emulator_answers_on() {
    let emulator = emulator();
    let cluster_id = launch(
        &emulator.router,
        "master node",
        json!({ "ReleaseLabel": "emr-7.9.0" }),
    )
    .await;
    tokio::time::sleep(PAST_CLUSTER_READY).await;

    let cases: Vec<(&str, &str, Option<&str>)> = vec![
        ("an address and a port", "127.0.0.1:9297", Some("127.0.0.1")),
        ("an address alone", "127.0.0.1", Some("127.0.0.1")),
        ("a loopback name", "localhost:9297", Some("localhost")),
        (
            "a client that resolves AWS's own hosts",
            "elasticmapreduce.us-west-2.amazonaws.com",
            None,
        ),
    ];
    for (label, host, expected) in &cases {
        let cluster = emr_at(
            &emulator.router,
            host,
            "DescribeCluster",
            json!({ "ClusterId": &cluster_id }),
        )
        .await["Cluster"]
            .clone();
        assert_eq!(
            cluster["Status"]["State"].as_str(),
            Some("WAITING"),
            "case {label:?}: the cluster must be up before its node is named, got {cluster}"
        );
        let named = cluster["MasterPublicDnsName"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: a cluster that is up names its master node, got {cluster}"))
            .to_string();
        match expected {
            Some(own) => assert_eq!(
                named, *own,
                "case {label:?}: expected the host this emulator answers on, got {named:?}"
            ),
            // The Amazon-provided private hostname form: `ip-<address>` and the Region's
            // private DNS domain.
            None => assert!(
                named.starts_with("ip-") && named.ends_with(".us-west-2.compute.internal"),
                "case {label:?}: expected the Amazon-provided private hostname, got {named:?}"
            ),
        }
    }
}

/// THE RULE for graceful replacement of degraded core nodes: a cluster with more than one
/// primary node is not offered it at all, and every other cluster runs with the launch's own
/// value or the service's default.
///
/// The rows span what the rule generalises over: how many primary nodes the cluster has (one
/// and several), which launch form states them (the uniform form, instance groups, instance
/// fleets), and what the launch asked for (nothing, off, on) — so the primary count is what
/// decides it rather than the request alone.
#[tokio::test]
async fn a_highly_available_cluster_is_not_offered_unhealthy_node_replacement() {
    let cases: Vec<(&str, Value, bool)> = vec![
        (
            "one primary, nothing asked for",
            json!({ "InstanceCount": 1, "MasterInstanceType": "m5.xlarge" }),
            true,
        ),
        (
            "one primary, asked to turn it off",
            json!({
                "InstanceCount": 1,
                "MasterInstanceType": "m5.xlarge",
                "UnhealthyNodeReplacement": false,
            }),
            false,
        ),
        (
            "one primary and a core node, asked to turn it on",
            json!({
                "InstanceCount": 2,
                "MasterInstanceType": "m5.xlarge",
                "SlaveInstanceType": "m5.xlarge",
                "UnhealthyNodeReplacement": true,
            }),
            true,
        ),
        (
            "three primaries in instance groups, nothing asked for",
            json!({
                "InstanceGroups": [
                    { "InstanceRole": "MASTER", "InstanceType": "m5.xlarge", "InstanceCount": 3 },
                    { "InstanceRole": "CORE", "InstanceType": "m5.xlarge", "InstanceCount": 1 },
                ],
            }),
            false,
        ),
        (
            "three primaries in instance groups, asked to turn it on",
            json!({
                "InstanceGroups": [
                    { "InstanceRole": "MASTER", "InstanceType": "m5.xlarge", "InstanceCount": 3 },
                ],
                "UnhealthyNodeReplacement": true,
            }),
            false,
        ),
        (
            "two primaries in an instance fleet",
            json!({
                "InstanceFleets": [{
                    "InstanceFleetType": "MASTER",
                    "TargetOnDemandCapacity": 2,
                    "InstanceTypeConfigs": [{ "InstanceType": "m5.xlarge" }],
                }],
            }),
            false,
        ),
    ];

    let emulator = emulator();
    for (label, instances, expected) in &cases {
        let mut request = instances.clone();
        request["KeepJobFlowAliveWhenNoSteps"] = json!(true);
        let cluster_id = launch(
            &emulator.router,
            label,
            json!({ "ReleaseLabel": "emr-7.9.0", "Instances": request }),
        )
        .await;
        let cluster = described(&emulator.router, &cluster_id).await;
        assert_eq!(
            cluster["UnhealthyNodeReplacement"].as_bool(),
            Some(*expected),
            "case {label:?}: expected UnhealthyNodeReplacement {expected}, got {cluster}"
        );
    }
}

/// THE RULE for an application's version: a cluster reports the version the RELEASE it
/// launched on ships.
///
/// The rows are the catalog's own boundaries for Spark on the 7.x series — the release each
/// version is carried from — so a cluster on a release reports that release's version rather
/// than the newest the catalog holds.
#[tokio::test]
async fn a_cluster_reports_the_application_version_its_release_ships() {
    // CATALOG DATA, from the Amazon EMR Release Guide's release-version table.
    let cases: [(&str, &str); 3] = [
        ("emr-7.5.0", "3.5.2"),
        ("emr-7.6.0", "3.5.4"),
        ("emr-7.9.0", "3.5.5"),
    ];
    let emulator = emulator();
    for (release, version) in &cases {
        let cluster_id = launch(
            &emulator.router,
            release,
            json!({ "ReleaseLabel": release, "Applications": [{ "Name": "Spark" }] }),
        )
        .await;
        let cluster = described(&emulator.router, &cluster_id).await;
        let reported = cluster["Applications"][0]["Version"].as_str();
        assert_eq!(
            reported,
            Some(*version),
            "case {release:?}: expected Spark {version}, got {cluster}"
        );
        // `DescribeReleaseLabel` publishes the same version for that release: one catalog,
        // two readers.
        let published = emr(
            &emulator.router,
            "DescribeReleaseLabel",
            json!({ "ReleaseLabel": release }),
        )
        .await;
        let catalogued = published["Applications"]
            .as_array()
            .and_then(|applications| {
                applications
                    .iter()
                    .find(|application| application["Name"].as_str() == Some("Spark"))
                    .cloned()
            })
            .unwrap_or_else(|| panic!("case {release:?}: the release publishes Spark"));
        assert_eq!(
            catalogued["Version"].as_str(),
            Some(*version),
            "case {release:?}: the release catalog publishes the same version, got {catalogued}"
        );
    }
}
