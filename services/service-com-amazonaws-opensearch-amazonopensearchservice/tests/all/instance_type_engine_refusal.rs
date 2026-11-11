// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the refusal an instance type the engine release in play does
//! not offer is answered with, on every path that pairs the two:
//! `DescribeInstanceTypeLimits`, `CreateDomain` and `UpdateDomainConfig`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`]; this suite adds no second copy of it.
//!
//! The rule under test: an instance type the named engine release does not offer is
//! refused with the modelled `InvalidTypeException` (`errorKind` client, `httpError`
//! 409, from `smithy_query(query="shape",shape="InvalidTypeException")`), whose
//! message is the fixed text `Invalid instance type: ` followed by the instance type
//! the request named — and nothing else: not the release the pairing was asked on.
//! The same wording answers the same pairing whichever operation was asked, and a
//! refused create or update stores nothing.
//!
//! So the assertions span SETS of inputs: a table of pairings covering every instance
//! family the published catalog gates on an engine release, on both engine lines,
//! each row's premise checked against the catalog the service itself publishes; the
//! releases that DO offer those families, which answer normally; and every tier of a
//! cluster configuration that names an instance type, since a domain runs one engine
//! release for all of its nodes.

use serde_json::{json, Value};

use super::domain_lifecycle::{
    error_code, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS, SETTLE_WAIT,
};

/// Pairings the service does not offer: an instance type together with an engine
/// release that does not run it, one row per family the published instance-type
/// catalog gates on a release (the UltraWarm family, the Graviton families and the
/// OpenSearch-only family), each asked on a release before its gate and, where the
/// family is gated across both engine lines, on each line.
///
/// Source of the rows: the catalog `ListInstanceTypeDetails` publishes, which
/// [`the_published_catalog_agrees_about_every_row`] below reads for each row rather
/// than this suite asserting the gates against themselves.
const UNOFFERED: [(&str, &str); 8] = [
    ("Elasticsearch_5.1", "ultrawarm1.medium.search"),
    ("Elasticsearch_6.7", "ultrawarm1.xlarge.search"),
    ("Elasticsearch_7.1", "c6g.large.search"),
    ("Elasticsearch_7.8", "r6g.2xlarge.search"),
    ("Elasticsearch_7.8", "t4g.small.search"),
    ("Elasticsearch_7.10", "or1.large.search"),
    ("OpenSearch_1.3", "or1.medium.search"),
    ("OpenSearch_2.3", "or1.medium.search"),
];

/// Pairings the service DOES offer, as the counterpart of each row above: the release
/// each gated family is offered from, a later release of the same line, and an
/// ungated family on an early release.
const OFFERED: [(&str, &str); 6] = [
    ("Elasticsearch_6.8", "ultrawarm1.medium.search"),
    ("Elasticsearch_7.9", "c6g.large.search"),
    ("Elasticsearch_7.10", "t4g.small.search"),
    ("OpenSearch_2.11", "or1.medium.search"),
    ("OpenSearch_2.19", "or1.large.search"),
    ("Elasticsearch_5.1", "m5.large.search"),
];

/// This suite's OWN statement of the refusal text: the fixed sentence, whose single
/// variable part is the instance type the request named. Restated here rather than
/// imported from the service, so changing it in the service fails this suite instead
/// of being asserted against itself.
fn expected_message(instance_type: &str) -> String {
    format!("Invalid instance type: {instance_type}")
}

/// The error message of a refused answer, with the case named on failure.
fn message(answer: &Answer, case: &str) -> String {
    answer.body["message"]
        .as_str()
        .or_else(|| answer.body["Message"].as_str())
        .unwrap_or_else(|| panic!("case {case}: expected an error message, got {:?}", answer.body))
        .to_owned()
}

/// Assert one answer IS the refusal the rule prescribes for `instance_type`.
fn assert_refused(answer: &Answer, instance_type: &str, case: &str) {
    assert_eq!(
        answer.status, 409,
        "case {case}: expected HTTP 409, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        error_code(answer),
        "InvalidTypeException",
        "case {case}: expected the modelled unsupported-type refusal, got {:?}",
        answer.body
    );
    let actual = message(answer, case);
    assert_eq!(
        actual,
        expected_message(instance_type),
        "case {case}: expected the message to name the instance type alone, got {actual:?}"
    );
}

/// `DescribeInstanceTypeLimits`:
/// `GET /2021-01-01/opensearch/instanceTypeLimits/{EngineVersion}/{InstanceType}`.
async fn describe_limits(emulator: &Emulator, engine_version: &str, instance_type: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/instanceTypeLimits/{engine_version}/{instance_type}"),
            None,
        )
        .await
}

/// Every instance type the catalog publishes for one release, read to the end of its
/// pagination: `GET /2021-01-01/opensearch/instanceTypeDetails/{EngineVersion}`.
async fn offered_types(emulator: &Emulator, engine_version: &str) -> Vec<String> {
    let mut types: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let uri = match token.as_deref() {
            None => format!("{PREFIX}/opensearch/instanceTypeDetails/{engine_version}"),
            Some(next) => {
                format!("{PREFIX}/opensearch/instanceTypeDetails/{engine_version}?nextToken={next}")
            }
        };
        let answer = emulator.call("GET", &uri, None).await;
        assert_eq!(
            answer.status, 200,
            "case ListInstanceTypeDetails({engine_version}, token={token:?}): expected HTTP 200, \
             got {} {:?}",
            answer.status, answer.body
        );
        types.extend(
            answer.body["InstanceTypeDetails"]
                .as_array()
                .unwrap_or(&Vec::new())
                .iter()
                .filter_map(|entry| entry["InstanceType"].as_str().map(str::to_owned)),
        );
        token = answer.body["NextToken"].as_str().map(str::to_owned);
        if token.is_none() {
            break;
        }
    }
    types
}

/// A domain name of the modelled shape (`^[a-z][a-z0-9\-]+$`, 3..28) for one row.
fn domain_name(prefix: &str, row: usize) -> String {
    format!("{prefix}-{row}")
}

#[tokio::test]
async fn the_published_catalog_agrees_about_every_row() {
    let emulator = Emulator::new();

    for (version, instance_type) in UNOFFERED {
        let case = format!("{version} {instance_type}");
        let offered = offered_types(&emulator, version).await;
        assert!(
            !offered.contains(&instance_type.to_string()),
            "case {case}: the row's premise is that the release does not offer the type, \
             but the published catalog lists it: {offered:?}"
        );
    }

    for (version, instance_type) in OFFERED {
        let case = format!("{version} {instance_type}");
        let offered = offered_types(&emulator, version).await;
        assert!(
            offered.contains(&instance_type.to_string()),
            "case {case}: the row's premise is that the release offers the type, but the \
             published catalog does not list it: {offered:?}"
        );
    }
}

#[tokio::test]
async fn the_limits_read_refuses_an_unoffered_pairing_by_naming_the_type_alone() {
    let emulator = Emulator::new();

    for (version, instance_type) in UNOFFERED {
        let case = format!("DescribeInstanceTypeLimits({version}, {instance_type})");
        let answer = describe_limits(&emulator, version, instance_type).await;
        assert_refused(&answer, instance_type, &case);
        assert!(
            !message(&answer, &case).contains(version),
            "case {case}: the refusal names the instance type alone, not the release, got {:?}",
            answer.body
        );
    }

    for (version, instance_type) in OFFERED {
        let case = format!("DescribeInstanceTypeLimits({version}, {instance_type})");
        let answer = describe_limits(&emulator, version, instance_type).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected a release that offers the type to answer, got {} {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.body["LimitsByRole"].is_object(),
            "case {case}: expected a LimitsByRole map, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn create_refuses_the_same_pairing_with_the_same_text_and_stores_nothing() {
    let emulator = Emulator::new();

    for (row, (version, instance_type)) in UNOFFERED.into_iter().enumerate() {
        let name = domain_name("create-unoffered", row);
        let case = format!("CreateDomain({name}, {version}, {instance_type})");
        let answer = emulator
            .create(json!({
                "DomainName": name,
                "EngineVersion": version,
                "ClusterConfig": { "InstanceType": instance_type, "InstanceCount": 2 },
            }))
            .await;
        assert_refused(&answer, instance_type, &case);

        // A refused create mints nothing: the name is still free of any record.
        let read = emulator.describe(&name).await;
        assert_eq!(
            read.status, RECORDED_NOT_FOUND_STATUS,
            "case {case}: a refused create stores no domain, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            error_code(&read),
            "ResourceNotFoundException",
            "case {case}: a refused create stores no domain, got {:?}",
            read.body
        );
    }

    for (row, (version, instance_type)) in OFFERED.into_iter().enumerate() {
        let name = domain_name("create-offered", row);
        let case = format!("CreateDomain({name}, {version}, {instance_type})");
        let answer = emulator
            .create(json!({
                "DomainName": name,
                "EngineVersion": version,
                "ClusterConfig": { "InstanceType": instance_type, "InstanceCount": 2 },
            }))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected a release that offers the type to create, got {} {:?}",
            answer.status, answer.body
        );
        // Round trip: the created domain reads back on the type and release it was
        // created with.
        let read = emulator.describe(&name).await;
        assert_eq!(
            read.body["DomainStatus"]["ClusterConfig"]["InstanceType"],
            json!(instance_type),
            "case {case}: expected the stored cluster to run the created type, got {:?}",
            read.body["DomainStatus"]["ClusterConfig"]
        );
        assert_eq!(
            read.body["DomainStatus"]["EngineVersion"],
            json!(version),
            "case {case}: expected the stored domain to run the created release, got {:?}",
            read.body["DomainStatus"]
        );
    }
}

/// One cluster configuration per tier that names an instance type, each pairing an
/// offered data-node type with an unoffered type in the tier under test: the
/// dedicated master nodes, the warm tier and a node option. The warm tier's member
/// targets `OpenSearchWarmPartitionInstanceType`, so its row names a type of that
/// enum (source:
/// `smithy_query(query="shape",shape="OpenSearchWarmPartitionInstanceType")`), and a
/// node option's type is `NodeOptionsNodeType`'s one value, `coordinator`.
fn tier_rows() -> Vec<(&'static str, &'static str, &'static str, Value)> {
    vec![
        (
            "the data nodes",
            "OpenSearch_2.3",
            "or1.medium.search",
            json!({ "InstanceType": "or1.medium.search", "InstanceCount": 2 }),
        ),
        (
            "the dedicated master nodes",
            "Elasticsearch_7.8",
            "r6g.large.search",
            json!({
                "InstanceType": "m5.large.search",
                "InstanceCount": 2,
                "DedicatedMasterEnabled": true,
                "DedicatedMasterType": "r6g.large.search",
                "DedicatedMasterCount": 3,
            }),
        ),
        (
            "the warm tier",
            "Elasticsearch_6.7",
            "ultrawarm1.medium.search",
            json!({
                "InstanceType": "m5.large.search",
                "InstanceCount": 2,
                "WarmEnabled": true,
                "WarmType": "ultrawarm1.medium.search",
                "WarmCount": 2,
            }),
        ),
        (
            "a node option",
            "OpenSearch_2.3",
            "or1.large.search",
            json!({
                "InstanceType": "m5.large.search",
                "InstanceCount": 2,
                "NodeOptions": [{
                    "NodeType": "coordinator",
                    "NodeConfig": { "Enabled": true, "Type": "or1.large.search", "Count": 2 },
                }],
            }),
        ),
    ]
}

#[tokio::test]
async fn every_tier_the_cluster_provisions_is_scoped_to_the_release() {
    let emulator = Emulator::new();

    for (row, (tier, version, instance_type, cluster)) in tier_rows().into_iter().enumerate() {
        let name = domain_name("tier", row);
        let case = format!("CreateDomain({name}, {version}, {tier})");
        let answer = emulator
            .create(json!({
                "DomainName": name,
                "EngineVersion": version,
                "ClusterConfig": cluster,
            }))
            .await;
        assert_refused(&answer, instance_type, &case);
    }
}

#[tokio::test]
async fn a_tier_that_provisions_no_node_pairs_nothing_with_the_release() {
    let emulator = Emulator::new();

    // `DedicatedMasterType` goes with "indicates whether dedicated master nodes are
    // enabled for the cluster", `WarmType` with "whether to enable warm storage for the
    // cluster" and a node option's type with "whether a specific node type is active or
    // inactive" (source: `smithy_query(query="shape",shape="ClusterConfig,NodeConfig")`),
    // so a type named beside a tier that is turned off provisions no node and pairs no
    // instance with the release.
    let rows: [(&str, Value); 3] = [
        (
            "dedicated masters off",
            json!({
                "InstanceType": "m5.large.search",
                "InstanceCount": 2,
                "DedicatedMasterEnabled": false,
                "DedicatedMasterType": "r6g.large.search",
            }),
        ),
        (
            "warm storage off",
            json!({
                "InstanceType": "m5.large.search",
                "InstanceCount": 2,
                "WarmEnabled": false,
                "WarmType": "ultrawarm1.medium.search",
            }),
        ),
        (
            "an inactive node option",
            json!({
                "InstanceType": "m5.large.search",
                "InstanceCount": 2,
                "NodeOptions": [{
                    "NodeType": "coordinator",
                    "NodeConfig": { "Enabled": false, "Type": "or1.large.search", "Count": 2 },
                }],
            }),
        ),
    ];

    for (row, (label, cluster)) in rows.into_iter().enumerate() {
        let name = domain_name("inactive", row);
        let version = "Elasticsearch_7.8";
        let case = format!("CreateDomain({name}, {version}, {label})");
        let answer = emulator
            .create(json!({
                "DomainName": name,
                "EngineVersion": version,
                "ClusterConfig": cluster,
            }))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected a tier that provisions no node to pair nothing with the \
             release, got {} {:?}",
            answer.status, answer.body
        );
    }
}

#[tokio::test]
async fn an_update_refuses_the_same_pairing_and_keeps_the_stored_cluster() {
    let emulator = Emulator::new();

    // The release a domain runs is not a member of `UpdateDomainConfig`, so the
    // pairing an update is judged on is the merged cluster against the release the
    // domain was created with.
    let rows: [(&str, &str, &str); 3] = [
        ("Elasticsearch_7.8", "m5.large.search", "c6g.large.search"),
        ("OpenSearch_2.3", "m5.large.search", "or1.medium.search"),
        ("Elasticsearch_6.7", "m5.large.search", "ultrawarm1.large.search"),
    ];

    for (row, (version, created_type, unoffered_type)) in rows.into_iter().enumerate() {
        let name = domain_name("update-unoffered", row);
        let created = emulator
            .create(json!({
                "DomainName": name,
                "EngineVersion": version,
                "ClusterConfig": { "InstanceType": created_type, "InstanceCount": 2 },
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case CreateDomain({name}, {version}, {created_type}): expected 200, got {} {:?}",
            created.status, created.body
        );
        // The service applies one change of a domain at a time, so the change the
        // create started has to land before an update can start another.
        tokio::time::sleep(SETTLE_WAIT).await;

        // An applied update and a dry run are both refused: the dry run reports what
        // applying the change would do, and this change cannot be applied.
        for dry_run in [false, true] {
            let case = format!("UpdateDomainConfig({name}, {unoffered_type}, dryRun={dry_run})");
            let answer = emulator
                .update_config(
                    &name,
                    json!({
                        "ClusterConfig": { "InstanceType": unoffered_type, "InstanceCount": 2 },
                        "DryRun": dry_run,
                    }),
                )
                .await;
            assert_refused(&answer, unoffered_type, &case);

            let stored = emulator.describe_config(&name).await;
            assert_eq!(
                stored.body["DomainConfig"]["ClusterConfig"]["Options"]["InstanceType"],
                json!(created_type),
                "case {case}: a refused update leaves the stored cluster alone, got {:?}",
                stored.body["DomainConfig"]["ClusterConfig"]
            );
        }

        // A change to a type the release DOES offer still applies, so the refusal is
        // the pairing's and not the update path's.
        let case = format!("UpdateDomainConfig({name}, t3.small.search)");
        let answer = emulator
            .update_config(
                &name,
                json!({ "ClusterConfig": { "InstanceType": "t3.small.search", "InstanceCount": 2 } }),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected a type the release offers to apply, got {} {:?}",
            answer.status, answer.body
        );
        let stored = emulator.describe_config(&name).await;
        assert_eq!(
            stored.body["DomainConfig"]["ClusterConfig"]["Options"]["InstanceType"],
            json!("t3.small.search"),
            "case {case}: expected the applied change to be stored, got {:?}",
            stored.body["DomainConfig"]["ClusterConfig"]
        );
    }
}
