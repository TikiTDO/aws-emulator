// SPDX-License-Identifier: CC0-1.0

//! The scalars this service's JSON reader coerces: a body member supplied in
//! another of JSON's scalar types than the one the model declares for it.
//!
//! RULE: such a request is read as though it carried the modelled scalar type —
//! it succeeds, and the cluster reports the number, boolean or string that value
//! states. The rule is a property of the reader, not of one member or one
//! operation, so the table below spans the MEMBER TYPE (boolean, integer,
//! string), the DIRECTION of the restatement (a number or boolean spelled as a
//! string, a string read off a number or a boolean), the DEPTH the value sits at
//! (the body itself, a nested structure, a structure inside a list, a map value),
//! how many values one body carries, and the operation the body was sent to.
//!
//! Its two edges are under test in the same table:
//!
//! * a member whose modelled type IS a string keeps the string it was sent, even
//!   when the text spells a number — so a cluster named `12345` is a cluster
//!   named `12345`;
//! * a string that spells no number or boolean at all is still refused, with
//!   nothing created, because there is no value in it to read.
//!
//! The last suite covers the one member this service reads without regard to
//! case, `kubernetesNetworkConfig.ipFamily`: the same leniency, applied to a
//! value set rather than to a JSON type.

use service_com_amazonaws_eks_awswesleyfrontend::state::CLUSTER_ACTIVATION_DELAY;

use crate::cluster_lifecycle::{create, delete, describe, minimal_request, REGION};
use crate::wire::{call, emulator, encode_label};

/// A request with `members` merged into the smallest accepted body.
fn request(name: &str, members: serde_json::Value) -> serde_json::Value {
    let mut body = minimal_request(name);
    let (Some(body_members), Some(extra)) = (body.as_object_mut(), members.as_object()) else {
        panic!("both the request and the members merged into it must be objects");
    };
    for (key, value) in extra {
        body_members.insert(key.clone(), value.clone());
    }
    body
}

/// One row: a request carrying string-spelled scalars, and what the cluster must
/// report for each of them.
struct Row {
    case: &'static str,
    cluster: &'static str,
    body: serde_json::Value,
    expected: Vec<(&'static str, serde_json::Value)>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a boolean of the body itself",
            cluster: "coerce-top-boolean",
            body: serde_json::json!({ "deletionProtection": "true" }),
            expected: vec![("/deletionProtection", serde_json::json!(true))],
        },
        Row {
            case: "a boolean of a nested structure",
            cluster: "coerce-nested-boolean",
            body: serde_json::json!({
                "resourcesVpcConfig": {
                    "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"],
                    "endpointPrivateAccess": "true",
                    "endpointPublicAccess": "false",
                },
            }),
            expected: vec![
                (
                    "/resourcesVpcConfig/endpointPrivateAccess",
                    serde_json::json!(true),
                ),
                (
                    "/resourcesVpcConfig/endpointPublicAccess",
                    serde_json::json!(false),
                ),
            ],
        },
        Row {
            case: "a boolean two structures down",
            cluster: "coerce-deep-boolean",
            body: serde_json::json!({
                // The load balancing capability is one of the three EKS Auto Mode
                // capabilities Amazon EKS requires to be enabled together, so the
                // row enables all three — the string-spelled boolean under test is
                // still the one two structures down.
                "kubernetesNetworkConfig": { "elasticLoadBalancing": { "enabled": "true" } },
                "computeConfig": {
                    "enabled": true,
                    "nodePools": ["general-purpose"],
                    "nodeRoleArn": "arn:aws:iam::000000000000:role/eksNodeRole",
                },
                "storageConfig": { "blockStorage": { "enabled": true } },
            }),
            expected: vec![(
                "/kubernetesNetworkConfig/elasticLoadBalancing/enabled",
                serde_json::json!(true),
            )],
        },
        Row {
            case: "two integers of a nested structure",
            cluster: "coerce-integers",
            body: serde_json::json!({
                "kubeApiServerConfig": {
                    "serviceNodePortRange": { "minPort": "30100", "maxPort": "32100" },
                },
            }),
            expected: vec![(
                "/kubeApiServerConfig/serviceNodePortRange",
                serde_json::json!({ "minPort": 30100, "maxPort": 32100 }),
            )],
        },
        Row {
            case: "an integer inside a list element",
            cluster: "coerce-list-integer",
            body: serde_json::json!({
                "kubeSchedulerConfig": {
                    "nodeResourcesFit": {
                        "scoringStrategy": {
                            "type": "LeastAllocated",
                            "resources": [
                                { "name": "cpu", "weight": "7" },
                                { "name": "memory", "weight": "3" },
                            ],
                        },
                    },
                },
            }),
            expected: vec![(
                "/kubeSchedulerConfig/nodeResourcesFit/scoringStrategy/resources",
                serde_json::json!([
                    { "name": "cpu", "weight": 7 },
                    { "name": "memory", "weight": 3 },
                ]),
            )],
        },
        Row {
            case: "a boolean inside a list element",
            cluster: "coerce-list-boolean",
            body: serde_json::json!({
                "logging": {
                    "clusterLogging": [{ "types": ["audit"], "enabled": "true" }],
                },
            }),
            expected: vec![(
                "/logging/clusterLogging/0",
                serde_json::json!({ "types": ["audit"], "enabled": true }),
            )],
        },
        Row {
            case: "scalars at three depths in one body",
            cluster: "coerce-several",
            body: serde_json::json!({
                "deletionProtection": "false",
                "bootstrapSelfManagedAddons": "false",
                "zonalShiftConfig": { "enabled": "true" },
                "kubeControllerManagerConfig": {
                    "podGcControllerConfig": { "terminatedPodGcThreshold": "99" },
                },
            }),
            expected: vec![
                ("/deletionProtection", serde_json::json!(false)),
                ("/zonalShiftConfig/enabled", serde_json::json!(true)),
                (
                    "/kubeControllerManagerConfig/podGcControllerConfig\
                     /terminatedPodGcThreshold",
                    serde_json::json!(99),
                ),
            ],
        },
        Row {
            case: "a string member sent as a number",
            cluster: "coerce-string-member",
            body: serde_json::json!({ "version": 1.33 }),
            expected: vec![("/version", serde_json::json!("1.33"))],
        },
        Row {
            case: "a map value sent as a number and as a boolean",
            cluster: "coerce-map-values",
            // Two tags, well inside the modelled `TagMap` ceiling of 50, so the row
            // exercises the reader and not the tag bound.
            body: serde_json::json!({ "tags": { "count": 12, "enabled": true } }),
            expected: vec![(
                "/tags",
                serde_json::json!({ "count": "12", "enabled": "true" }),
            )],
        },
        Row {
            case: "a string member whose text spells a number keeps the string",
            cluster: "12345",
            body: serde_json::json!({ "deletionProtection": "true" }),
            expected: vec![
                ("/name", serde_json::json!("12345")),
                ("/deletionProtection", serde_json::json!(true)),
            ],
        },
        Row {
            case: "a tag value that spells a boolean keeps the string",
            cluster: "coerce-tag-value",
            body: serde_json::json!({
                "deletionProtection": "true",
                "tags": { "enabled": "false", "count": "12" },
            }),
            expected: vec![
                ("/deletionProtection", serde_json::json!(true)),
                (
                    "/tags",
                    serde_json::json!({ "enabled": "false", "count": "12" }),
                ),
            ],
        },
    ]
}

#[tokio::test]
async fn a_scalar_sent_as_a_string_is_read_as_the_scalar_it_spells() {
    let router = emulator();
    let rows = rows();

    for row in &rows {
        let created = create(&router, &request(row.cluster, row.body.clone())).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: a request AWS reads must be served, got {created:?}",
            row.case
        );
        assert_eq!(
            created.cluster()["status"],
            serde_json::json!("CREATING"),
            "case {:?}: the created cluster is CREATING, got {:?}",
            row.case,
            created.cluster()["status"]
        );
        for (pointer, expected) in &row.expected {
            let actual = created.cluster().pointer(pointer).cloned();
            assert_eq!(
                actual.as_ref(),
                Some(expected),
                "case {:?}: expected {pointer} to be read as {expected:?}, got {actual:?}",
                row.case
            );
        }

        // THE ROUND TRIP: the cluster was really created, and reads back carrying
        // the same values.
        let described = describe(&router, row.cluster).await;
        assert_eq!(
            described.status, 200,
            "case {:?}: the created cluster must read back, got {described:?}",
            row.case
        );
        for (pointer, expected) in &row.expected {
            let actual = described.cluster().pointer(pointer).cloned();
            assert_eq!(
                actual.as_ref(),
                Some(expected),
                "case {:?}: expected the stored {pointer} to be {expected:?}, got {actual:?}",
                row.case
            );
        }

        // And the teardown the client makes next finds it: a cluster the reader
        // accepted is a cluster the delete can address. A row that read
        // `deletionProtection` as `true` is refused instead — which is the same
        // rule once more, read through BEHAVIOUR rather than through an echo: the
        // string was read as the boolean the handler then acted on.
        let protected =
            created.cluster()["deletionProtection"] == serde_json::json!(true);
        let deleted = delete(&router, row.cluster).await;
        if protected {
            assert_eq!(
                (deleted.status, deleted.error_type.as_deref()),
                (400, Some("InvalidRequestException")),
                "case {:?}: a cluster whose deletionProtection was read as true cannot \
                 be deleted, got {deleted:?}",
                row.case
            );
            continue;
        }
        assert_eq!(
            deleted.status, 200,
            "case {:?}: the cluster the request created must be deletable, got {deleted:?}",
            row.case
        );
        assert_eq!(
            deleted.cluster()["status"],
            serde_json::json!("DELETING"),
            "case {:?}: the deleted cluster is DELETING, got {:?}",
            row.case,
            deleted.cluster()["status"]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: the reader is the SERVICE's, not one operation's — a body sent to any
// operation that takes one is read the same way.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_same_reading_serves_every_operation_that_takes_a_body() {
    let router = emulator();
    let created = create(&router, &minimal_request("coerce-update")).await;
    assert_eq!(
        created.status, 200,
        "setup: expected 200 from CreateCluster, got {created:?}"
    );
    // A configuration change starts from an `ACTIVE` cluster, so the clock is
    // given the span it owes this one.
    tokio::time::sleep(CLUSTER_ACTIVATION_DELAY + std::time::Duration::from_millis(250)).await;

    // `UpdateClusterConfig`'s own body, with its boolean sent as a string.
    let started = call(
        &router,
        REGION,
        "POST",
        &format!("/clusters/{}/update-config", encode_label("coerce-update")),
        &serde_json::json!({ "deletionProtection": "true" }).to_string(),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "a configuration change whose boolean is a string must be served, got {started:?}"
    );
    assert_eq!(
        started.body["update"]["params"],
        serde_json::json!([{ "type": "DeletionProtection", "value": "true" }]),
        "the update must carry the value the request asked for, got {:?}",
        started.body["update"]
    );
}

// ---------------------------------------------------------------------------
// RULE: `kubernetesNetworkConfig.ipFamily` names one of the two families
// `IpFamily` declares, matched WITHOUT regard to case, and the cluster reports
// the spelling the model declares. A value that is no declared family in any case
// is refused, as every other enum-backed member of this service is.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_ip_family_is_read_without_regard_to_case() {
    let router = emulator();

    // Rows: both members of `IpFamily` (`smithy_query(query="shape",
    // shape="IpFamily")`: `ipv4 | ipv6`), each spelled the declared way and in
    // three other cases — the service reads the FAMILY, not the spelling.
    let accepted: Vec<(&str, &str, &str)> = vec![
        ("the declared spelling", "ipv4", "ipv4"),
        ("mixed case", "IPv4", "ipv4"),
        ("upper case", "IPV4", "ipv4"),
        ("capitalised", "Ipv4", "ipv4"),
        ("the declared spelling of the other family", "ipv6", "ipv6"),
        ("mixed case of the other family", "IPv6", "ipv6"),
        ("upper case of the other family", "IPV6", "ipv6"),
    ];

    for (index, (case, sent, declared)) in accepted.iter().enumerate() {
        let cluster = format!("ipfamily-{index}");
        let answer = create(
            &router,
            &request(
                &cluster,
                serde_json::json!({ "kubernetesNetworkConfig": { "ipFamily": sent } }),
            ),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?} ({sent:?}): the family is read whatever its case, got {answer:?}"
        );
        assert_eq!(
            answer.cluster()["kubernetesNetworkConfig"]["ipFamily"],
            serde_json::json!(declared),
            "case {case:?} ({sent:?}): the cluster reports the declared spelling, got {:?}",
            answer.cluster()["kubernetesNetworkConfig"]
        );
        // The family decides which service CIDR the cluster gets, so the value
        // reached the handler and not just the response.
        let network = &answer.cluster()["kubernetesNetworkConfig"];
        if *declared == "ipv6" {
            assert!(
                network["serviceIpv6Cidr"].is_string() && network["serviceIpv4Cidr"].is_null(),
                "case {case:?}: an ipv6 cluster gets the IPv6 service CIDR, got {network:?}"
            );
        } else {
            assert!(
                network["serviceIpv4Cidr"].is_string() && network["serviceIpv6Cidr"].is_null(),
                "case {case:?}: an ipv4 cluster gets the IPv4 service CIDR, got {network:?}"
            );
        }
        // And it reads back that way: the declared spelling is what was stored.
        let described = describe(&router, &cluster).await;
        assert_eq!(
            described.cluster()["kubernetesNetworkConfig"]["ipFamily"],
            serde_json::json!(declared),
            "case {case:?}: the stored family is the declared spelling, got {:?}",
            described.cluster()["kubernetesNetworkConfig"]
        );
    }

    // Rows differ along HOW the value misses the enum: a family that does not
    // exist, a declared one with padding, a near-miss, and the empty string.
    for (index, (case, sent)) in [
        ("a family the enum does not list", "ipv5"),
        ("a declared family with padding", " ipv4"),
        ("a near-miss of a declared family", "ip-v4"),
        ("the empty string", ""),
    ]
    .iter()
    .enumerate()
    {
        let cluster = format!("ipfamily-refused-{index}");
        let answer = create(
            &router,
            &request(
                &cluster,
                serde_json::json!({ "kubernetesNetworkConfig": { "ipFamily": sent } }),
            ),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_type.as_deref()),
            (400, Some("InvalidParameterException")),
            "case {case:?} ({sent:?}): a value that names no family is refused, got {answer:?}"
        );
        assert_eq!(
            describe(&router, &cluster).await.status,
            404,
            "case {case:?}: a refused create stores nothing"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a string that spells no number or boolean is still refused — there is no
// value in it to read — and nothing is created.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_string_that_spells_no_scalar_is_still_refused() {
    let router = emulator();

    // Rows differ along the text that spells no scalar: a word, a padded number,
    // a number with a unit, an empty string, and a boolean spelled the way a
    // shell would.
    let cases: Vec<(&str, serde_json::Value)> = vec![
        (
            "a word at an integer member",
            serde_json::json!({
                "kubeApiServerConfig": { "serviceNodePortRange": { "minPort": "many" } },
            }),
        ),
        (
            "a padded number at an integer member",
            serde_json::json!({
                "kubeApiServerConfig": { "serviceNodePortRange": { "minPort": " 30100" } },
            }),
        ),
        (
            "a number with a unit at an integer member",
            serde_json::json!({
                "kubeControllerManagerConfig": {
                    "podGcControllerConfig": { "terminatedPodGcThreshold": "99 pods" },
                },
            }),
        ),
        (
            "an empty string at a boolean member",
            serde_json::json!({ "deletionProtection": "" }),
        ),
        (
            "a boolean spelled the way a shell spells one",
            serde_json::json!({ "deletionProtection": "yes" }),
        ),
    ];

    for (case, members) in &cases {
        let answer = create(&router, &request("coerce-refused", members.clone())).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a value that spells no scalar must be refused, got {answer:?}"
        );
        assert!(
            answer.body["cluster"].is_null(),
            "case {case:?}: a refused request creates no cluster, got {:?}",
            answer.body
        );
        let described = describe(&router, "coerce-refused").await;
        assert_eq!(
            described.status, 404,
            "case {case:?}: a refused request must store nothing, got {described:?}"
        );
    }
}
