// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the EKS managed-capability slice: `CreateCapability`,
//! `DescribeCapability`, `ListCapabilities` and `DeleteCapability`, driven over the
//! wire through the same `MultiServiceRouter` the production binary mounts.
//!
//! Every suite here asserts a RULE over a TABLE of inputs rather than one recorded
//! request: the rows differ along what the rule generalises over (capability type,
//! which optional members are supplied, name shape and charset, page size, which
//! operation is called, which cluster status the call lands on), and each assertion
//! names its case and prints the actual value.

use std::collections::BTreeSet;
use std::time::Duration;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    cluster_not_found_message, CAPABILITY_ACTIVATION_DELAY, CLUSTER_ACTIVATION_DELAY,
    MAX_CAPABILITIES_PER_CLUSTER, RESOURCE_NOT_FOUND_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

// The harness every suite in this test binary shares, declared once in
// `tests/all/wire.rs`: the router under test, one request, and the answer reduced
// to what a client branches on.
use crate::wire::{call, emulator, encode_label, Answer};

const REGION: &str = "us-west-2";

/// The IAM role a capability "uses to interact with Amazon Web Services services".
const CAPABILITY_ROLE: &str = "arn:aws:iam::000000000000:role/eksCapabilityRole";

/// The `capability` member every capability operation answers with.
fn capability(answer: &Answer) -> &serde_json::Value {
    &answer.body["capability"]
}

/// The error code a client matches on, with the `shape#` prefix a restJson1
/// envelope may carry stripped.
fn code(answer: &Answer) -> String {
    answer
        .error_type
        .as_deref()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_owned()
}

async fn create_capability(
    router: &MultiServiceRouter,
    cluster: &str,
    body: &serde_json::Value,
) -> Answer {
    call(
        router,
        REGION,
        "POST",
        &format!("/clusters/{}/capabilities", encode_label(cluster)),
        &body.to_string(),
    )
    .await
}

async fn describe_capability(router: &MultiServiceRouter, cluster: &str, name: &str) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!(
            "/clusters/{}/capabilities/{}",
            encode_label(cluster),
            encode_label(name)
        ),
        "",
    )
    .await
}

async fn delete_capability(router: &MultiServiceRouter, cluster: &str, name: &str) -> Answer {
    call(
        router,
        REGION,
        "DELETE",
        &format!(
            "/clusters/{}/capabilities/{}",
            encode_label(cluster),
            encode_label(name)
        ),
        "",
    )
    .await
}

async fn list_capabilities(router: &MultiServiceRouter, cluster: &str, query: &str) -> Answer {
    let base = format!("/clusters/{}/capabilities", encode_label(cluster));
    let uri = if query.is_empty() {
        base
    } else {
        format!("{base}?{query}")
    };
    call(router, REGION, "GET", &uri, "").await
}

async fn describe_cluster(router: &MultiServiceRouter, cluster: &str) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!("/clusters/{}", encode_label(cluster)),
        "",
    )
    .await
}

/// Create every named cluster and wait out the one clock-owed `CREATING` -> `ACTIVE`
/// move, because a capability can only be created on an `ACTIVE` cluster.
async fn active_clusters(router: &MultiServiceRouter, names: &[&str]) {
    for name in names {
        let body = serde_json::json!({
            "name": name,
            "roleArn": "arn:aws:iam::000000000000:role/eksClusterRole",
            "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"] },
        });
        let created = call(router, REGION, "POST", "/clusters", &body.to_string()).await;
        assert_eq!(
            created.status, 200,
            "setup: expected 200 from CreateCluster for {name:?}, got {created:?}"
        );
    }
    tokio::time::sleep(CLUSTER_ACTIVATION_DELAY + Duration::from_millis(250)).await;
    for name in names {
        let described = describe_cluster(router, name).await;
        assert_eq!(
            described.body["cluster"]["status"],
            serde_json::json!("ACTIVE"),
            "setup: cluster {name:?} must be ACTIVE before capabilities are created, got {described:?}"
        );
    }
}

/// The smallest request `CreateCapability` accepts: the three required body members.
fn minimal_request(name: &str, capability_type: &str) -> serde_json::Value {
    serde_json::json!({
        "capabilityName": name,
        "type": capability_type,
        "roleArn": CAPABILITY_ROLE,
        "deletePropagationPolicy": "RETAIN",
    })
}

fn listed_names(answer: &Answer) -> Vec<String> {
    answer.body["capabilities"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    item["capabilityName"]
                        .as_str()
                        .map(str::to_owned)
                })
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// RULE: a capability read back is the record that was written — every member the
// request supplied, plus the server-generated ones, unchanged; and the listing
// agrees with it.
// ---------------------------------------------------------------------------

/// The table spans what the rule generalises over: the capability `type` (every
/// member of `CapabilityType`, from
/// `smithy_query query="shape" shape="CapabilityType"`), which optional members are
/// supplied at all, and the name shape. Each row gets its own cluster, because one
/// cluster holds at most one capability of each type.
#[tokio::test]
async fn a_created_capability_reads_back_exactly_as_stored() {
    let router = emulator();
    let clusters = ["minimal-cluster", "argocd-cluster", "ack-cluster"];
    active_clusters(&router, &clusters).await;

    let cases: Vec<(&str, &str, serde_json::Value)> = vec![
        (
            "only the required members, type KRO",
            "minimal-cluster",
            minimal_request("kro", "KRO"),
        ),
        (
            "every Argo CD configuration member, plus tags and a token",
            "argocd-cluster",
            serde_json::json!({
                "capabilityName": "Argo-CD_1",
                "type": "ARGOCD",
                "roleArn": "arn:aws:iam::000000000000:role/argoCdRole",
                "deletePropagationPolicy": "RETAIN",
                "clientRequestToken": "token-argocd",
                "tags": { "env": "dev", "team": "platform" },
                "configuration": {
                    "argoCd": {
                        "namespace": "argocd",
                        "awsIdc": {
                            "idcInstanceArn": "arn:aws:sso:::instance/ssoins-1234567890abcdef",
                            "idcRegion": "us-east-1",
                        },
                        "rbacRoleMappings": [
                            { "role": "ADMIN", "identities": [{ "id": "group-1", "type": "SSO_GROUP" }] },
                            { "role": "VIEWER", "identities": [{ "id": "user-1", "type": "SSO_USER" }] },
                        ],
                        "networkAccess": { "vpceIds": ["vpce-0123456789abcdef0"] },
                        "endpointPrefix": "argo-demo",
                    },
                },
            }),
        ),
        (
            "an ACK configuration with an explicitly empty disabledServices list",
            "ack-cluster",
            serde_json::json!({
                "capabilityName": "ack",
                "type": "ACK",
                "roleArn": CAPABILITY_ROLE,
                "deletePropagationPolicy": "RETAIN",
                "tags": { "only": "one" },
                "configuration": {
                    "ack": { "enableCrossNamespace": true, "disabledServices": [] },
                },
            }),
        ),
    ];

    for (case, cluster, request) in &cases {
        let name = request["capabilityName"]
            .as_str()
            .expect("the case names its capability");
        let created = create_capability(&router, cluster, request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateCapability, got {created:?}"
        );

        // The echo-back of everything the request supplied.
        for member in ["capabilityName", "type", "roleArn", "deletePropagationPolicy"] {
            assert_eq!(
                capability(&created)[member],
                request[member],
                "case {case:?}: expected {member} {:?}, got {:?}",
                request[member],
                capability(&created)[member]
            );
        }
        assert_eq!(
            capability(&created)["clusterName"],
            serde_json::json!(cluster),
            "case {case:?}: expected clusterName {cluster:?}, got {:?}",
            capability(&created)["clusterName"]
        );
        let expected_tags = request
            .get("tags")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        assert_eq!(
            capability(&created)["tags"],
            expected_tags,
            "case {case:?}: expected tags {expected_tags:?}, got {:?}",
            capability(&created)["tags"]
        );
        if let Some(argo_cd) = request["configuration"].get("argoCd") {
            let stored = &capability(&created)["configuration"]["argoCd"];
            for member in ["namespace", "endpointPrefix", "rbacRoleMappings", "networkAccess"] {
                assert_eq!(
                    stored[member], argo_cd[member],
                    "case {case:?}: expected configuration.argoCd.{member} {:?}, got {:?}",
                    argo_cd[member], stored[member]
                );
            }
            assert_eq!(
                stored["awsIdc"]["idcInstanceArn"], argo_cd["awsIdc"]["idcInstanceArn"],
                "case {case:?}: expected the supplied idcInstanceArn, got {stored:?}"
            );
            // This emulator runs no Argo CD server and creates no Identity Center
            // application, so neither server-generated member is invented.
            assert!(
                stored["serverUrl"].is_null()
                    && stored["awsIdc"]["idcManagedApplicationArn"].is_null(),
                "case {case:?}: no Argo CD server runs here, so no URL or IDC \
                 application ARN is reported, got {stored:?}"
            );
        }
        if let Some(ack) = request["configuration"].get("ack") {
            let stored = &capability(&created)["configuration"]["ack"];
            assert_eq!(
                stored["enableCrossNamespace"], ack["enableCrossNamespace"],
                "case {case:?}: expected the supplied enableCrossNamespace, got {stored:?}"
            );
            // "DescribeCapability returns the list exactly as you supplied it" —
            // including an empty list, which is not the same as omitting it.
            assert_eq!(
                stored["disabledServices"], ack["disabledServices"],
                "case {case:?}: expected disabledServices {:?}, got {:?}",
                ack["disabledServices"], stored["disabledServices"]
            );
        }

        // The server-generated members.
        let expected_arn = format!("arn:aws:eks:{REGION}:000000000000:capability/{cluster}/{name}");
        assert_eq!(
            capability(&created)["arn"],
            serde_json::json!(expected_arn),
            "case {case:?}: expected arn {expected_arn:?}, got {:?}",
            capability(&created)["arn"]
        );
        assert_eq!(
            capability(&created)["status"],
            serde_json::json!("CREATING"),
            "case {case:?}: a brand new capability is CREATING, got {:?}",
            capability(&created)["status"]
        );
        for member in ["createdAt", "modifiedAt"] {
            assert!(
                capability(&created)[member].is_number(),
                "case {case:?}: expected an epoch-seconds {member}, got {:?}",
                capability(&created)[member]
            );
        }
        assert_eq!(
            capability(&created)["health"]["issues"],
            serde_json::json!([]),
            "case {case:?}: a capability with nothing wrong reports an empty issue \
             list, got {:?}",
            capability(&created)["health"]
        );

        // THE ROUND TRIP: the describe is the same object, status aside (which the
        // clock may already have moved).
        let described = describe_capability(&router, cluster, name).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected 200 from DescribeCapability, got {described:?}"
        );
        let mut from_create = capability(&created).clone();
        let mut from_describe = capability(&described).clone();
        for value in [&mut from_create, &mut from_describe] {
            if let Some(object) = value.as_object_mut() {
                object.remove("status");
            }
        }
        assert_eq!(
            from_create, from_describe,
            "case {case:?}: DescribeCapability must return what CreateCapability stored"
        );

        // And the listing agrees with the description on the members a summary
        // carries.
        let listed = list_capabilities(&router, cluster, "").await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected 200 from ListCapabilities, got {listed:?}"
        );
        let summary = listed.body["capabilities"][0].clone();
        for member in ["capabilityName", "arn", "type", "createdAt", "modifiedAt"] {
            assert_eq!(
                summary[member],
                capability(&described)[member],
                "case {case:?}: ListCapabilities and DescribeCapability must agree on \
                 {member}; got {:?} and {:?}",
                summary[member],
                capability(&described)[member]
            );
        }
    }
}

/// `AckConfigRequest$enableCrossNamespace`: "If you don't specify this value, or you
/// omit the `ack` configuration entirely, the capability is created with this value
/// set to `false`". The rows are the three ways a request can leave it unset or set
/// it.
#[tokio::test]
async fn the_ack_cross_namespace_default_is_documented_and_materialised() {
    let router = emulator();
    let clusters = ["ack-omitted", "ack-empty", "ack-true", "ack-false"];
    active_clusters(&router, &clusters).await;

    let cases: Vec<(&str, &str, Option<serde_json::Value>, bool)> = vec![
        ("the ack configuration omitted entirely", "ack-omitted", None, false),
        (
            "an ack configuration naming neither member",
            "ack-empty",
            Some(serde_json::json!({ "ack": {} })),
            false,
        ),
        (
            "enableCrossNamespace true",
            "ack-true",
            Some(serde_json::json!({ "ack": { "enableCrossNamespace": true } })),
            true,
        ),
        (
            "enableCrossNamespace explicitly false",
            "ack-false",
            Some(serde_json::json!({ "ack": { "enableCrossNamespace": false } })),
            false,
        ),
    ];

    for (case, cluster, configuration, expected) in &cases {
        let mut request = minimal_request("ack", "ACK");
        if let Some(configuration) = configuration {
            request["configuration"] = configuration.clone();
        }
        let created = create_capability(&router, cluster, &request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateCapability, got {created:?}"
        );
        let described = describe_capability(&router, cluster, "ack").await;
        assert_eq!(
            capability(&described)["configuration"]["ack"]["enableCrossNamespace"],
            serde_json::json!(expected),
            "case {case:?}: expected enableCrossNamespace {expected}, got {:?}",
            capability(&described)["configuration"]["ack"]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: `capabilityName` is "a unique name ... within your cluster" that "can
// contain alphanumeric characters, hyphens, and underscores" — anything else is
// refused, and a name already taken conflicts.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn capability_names_are_accepted_or_refused_by_the_documented_charset() {
    let router = emulator();
    // Source for both tables: `CreateCapability$capabilityName`'s own
    // documentation (the shape carries no `@pattern`). The accepted rows walk the
    // whole documented charset — letters in both cases, digits, hyphen, underscore
    // — and a long name; the refused rows each break it in one way.
    let accepted = ["a", "9", "argocd", "ARGOCD", "Argo-CD_9", &"n".repeat(63)];
    let refused = [
        ("empty", ""),
        ("an inner space", "argo cd"),
        ("a dot", "argo.cd"),
        ("a slash", "argo/cd"),
        ("a colon", "argo:cd"),
        ("a non-ASCII letter", "argoçd"),
    ];

    let clusters: Vec<String> = (0..accepted.len())
        .map(|index| format!("charset-{index}"))
        .collect();
    let cluster_refs: Vec<&str> = clusters.iter().map(String::as_str).collect();
    active_clusters(&router, &cluster_refs).await;

    for (name, cluster) in accepted.iter().zip(&cluster_refs) {
        let created = create_capability(&router, cluster, &minimal_request(name, "KRO")).await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: a name of the documented charset is accepted, got {created:?}"
        );
        assert_eq!(
            capability(&created)["capabilityName"],
            serde_json::json!(name),
            "case {name:?}: expected the name echoed back, got {:?}",
            capability(&created)["capabilityName"]
        );
    }

    for (case, name) in &refused {
        let answer = create_capability(&router, cluster_refs[0], &minimal_request(name, "ACK")).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?} ({name:?}): expected 400, got {answer:?}"
        );
        assert_eq!(
            code(&answer),
            "InvalidParameterException",
            "case {case:?} ({name:?}): expected InvalidParameterException, got {answer:?}"
        );
        // Nothing was written: the cluster still holds only its accepted capability.
        let listed = listed_names(&list_capabilities(&router, cluster_refs[0], "").await);
        assert_eq!(
            listed,
            vec![accepted[0].to_string()],
            "case {case:?} ({name:?}): a refused create writes nothing, got {listed:?}"
        );
    }
}

/// "You can only create one Capability of each type on a given Amazon EKS cluster",
/// and "the name must be unique within your cluster" — so a cluster holds at most
/// one capability per `CapabilityType`.
#[tokio::test]
async fn one_capability_of_each_type_and_name_per_cluster() {
    let router = emulator();
    active_clusters(&router, &["conflicts", "other-cluster"]).await;

    // Source: `CapabilityType`'s members, from
    // `smithy_query query="shape" shape="CapabilityType"`.
    let types = ["ACK", "ARGOCD", "KRO"];
    assert_eq!(
        types.len(),
        MAX_CAPABILITIES_PER_CLUSTER,
        "the per-cluster ceiling is one capability per modelled type, got {}",
        MAX_CAPABILITIES_PER_CLUSTER
    );

    for capability_type in &types {
        let name = format!("first-{}", capability_type.to_ascii_lowercase());
        let created = create_capability(&router, "conflicts", &minimal_request(&name, capability_type)).await;
        assert_eq!(
            created.status, 200,
            "case {capability_type:?}: the cluster's first capability of a type is \
             created, got {created:?}"
        );
    }

    for capability_type in &types {
        // A SECOND capability of a type the cluster already holds, under a name it
        // does not: refused on the type.
        let name = format!("second-{}", capability_type.to_ascii_lowercase());
        let answer = create_capability(&router, "conflicts", &minimal_request(&name, capability_type)).await;
        assert_eq!(
            answer.status, 409,
            "case {capability_type:?}: a second capability of the same type is refused, \
             got {answer:?}"
        );
        assert_eq!(
            code(&answer),
            "ResourceInUseException",
            "case {capability_type:?}: expected ResourceInUseException, got {answer:?}"
        );

        // The SAME name again, with no idempotency token: refused on the name.
        let taken = format!("first-{}", capability_type.to_ascii_lowercase());
        let answer = create_capability(&router, "conflicts", &minimal_request(&taken, capability_type)).await;
        assert_eq!(
            answer.status, 409,
            "case {taken:?}: a name the cluster already holds is refused, got {answer:?}"
        );

        // The same type and name on ANOTHER cluster is a different capability:
        // uniqueness is per cluster.
        let elsewhere =
            create_capability(&router, "other-cluster", &minimal_request(&taken, capability_type)).await;
        assert_eq!(
            elsewhere.status, 200,
            "case {taken:?}: the same name and type on another cluster is independent, \
             got {elsewhere:?}"
        );
    }

    let listed = listed_names(&list_capabilities(&router, "conflicts", "").await);
    assert_eq!(
        listed.len(),
        MAX_CAPABILITIES_PER_CLUSTER,
        "a cluster holds at most one capability per type, got {listed:?}"
    );
}

/// `CreateCapability$configuration`: "The structure of this object varies depending
/// on the capability type." The rows are every (type, configuration member) pair.
#[tokio::test]
async fn a_configuration_member_belongs_to_exactly_one_capability_type() {
    let router = emulator();
    let argo_cd = serde_json::json!({
        "argoCd": { "awsIdc": { "idcInstanceArn": "arn:aws:sso:::instance/ssoins-1" } },
    });
    let ack = serde_json::json!({ "ack": { "enableCrossNamespace": true } });

    let cases: Vec<(&str, &str, &str, serde_json::Value, u16)> = vec![
        ("argoCd on ARGOCD", "match-argocd", "ARGOCD", argo_cd.clone(), 200),
        ("ack on ACK", "match-ack", "ACK", ack.clone(), 200),
        ("ack on ARGOCD", "cross-1", "ARGOCD", ack.clone(), 400),
        ("argoCd on ACK", "cross-2", "ACK", argo_cd.clone(), 400),
        ("argoCd on KRO", "cross-3", "KRO", argo_cd.clone(), 400),
        ("ack on KRO", "cross-4", "KRO", ack.clone(), 400),
    ];

    let clusters: Vec<&str> = cases.iter().map(|(_, cluster, ..)| *cluster).collect();
    active_clusters(&router, &clusters).await;

    for (case, cluster, capability_type, configuration, expected_status) in &cases {
        let mut request = minimal_request("configured", capability_type);
        request["configuration"] = configuration.clone();
        let answer = create_capability(&router, cluster, &request).await;
        assert_eq!(
            answer.status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {answer:?}"
        );
        if *expected_status == 400 {
            assert_eq!(
                code(&answer),
                "InvalidParameterException",
                "case {case:?}: expected InvalidParameterException, got {answer:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: a capability is addressed through its cluster, so a cluster this store does
// not hold has no capabilities to create, read, list or delete.
// ---------------------------------------------------------------------------

/// The rows are the four operations: a capability is addressed through its cluster,
/// so each reports a cluster this store does not hold with the SAME
/// `ResourceNotFoundException` (404) — `CreateCapability` and `ListCapabilities`
/// through the `extra_errors` widening of the shape the service models and their
/// sibling operations list.
#[tokio::test]
async fn every_capability_operation_refuses_an_unknown_cluster() {
    let router = emulator();
    active_clusters(&router, &["real-cluster"]).await;

    for cluster in ["no-such-cluster", "Another_Missing-9"] {
        let not_found = (404, "ResourceNotFoundException".to_string());
        let create = create_capability(&router, cluster, &minimal_request("any", "KRO")).await;
        assert_eq!(
            (create.status, code(&create)),
            not_found,
            "case {cluster:?}: CreateCapability on an unknown cluster, got {create:?}"
        );

        let describe = describe_capability(&router, cluster, "any").await;
        assert_eq!(
            (describe.status, code(&describe)),
            not_found,
            "case {cluster:?}: DescribeCapability on an unknown cluster, got {describe:?}"
        );

        let delete = delete_capability(&router, cluster, "any").await;
        assert_eq!(
            (delete.status, code(&delete)),
            not_found,
            "case {cluster:?}: DeleteCapability on an unknown cluster, got {delete:?}"
        );

        let listed = list_capabilities(&router, cluster, "").await;
        assert_eq!(
            (listed.status, code(&listed)),
            not_found,
            "case {cluster:?}: ListCapabilities on an unknown cluster, got {listed:?}"
        );

        // The parent rule wins over a malformed page token too: an unknown cluster
        // is that 404 whatever else is wrong with the request.
        let with_token = list_capabilities(&router, cluster, "nextToken=zzzz").await;
        assert_eq!(
            (with_token.status, code(&with_token)),
            not_found,
            "case {cluster:?}: ListCapabilities on an unknown cluster with a bad \
             token, got {with_token:?}"
        );

        // The two that only VALIDATE the cluster — the create and the listing — report
        // the cluster as the resource they could not resolve, so they name it.
        for (operation, answer) in [("CreateCapability", &create), ("ListCapabilities", &listed)] {
            assert_eq!(
                answer.body["clusterName"],
                serde_json::json!(cluster),
                "case {cluster:?}: {operation} must name the missing cluster, got {answer:?}"
            );
            assert_eq!(
                answer.body["message"],
                serde_json::json!(cluster_not_found_message(cluster)),
                "case {cluster:?}: {operation} must report the cluster it could not \
                 resolve, got {answer:?}"
            );
        }

        // The two that address ONE resource — the `(cluster, capability)` pair — report
        // the miss generically: no identity member at all, so a caller is not told
        // which half was absent.
        for (operation, answer) in [("DescribeCapability", &describe), ("DeleteCapability", &delete)]
        {
            assert_eq!(
                answer.body["message"],
                serde_json::json!(RESOURCE_NOT_FOUND_MESSAGE),
                "case {cluster:?}: {operation} must report the generic not-found \
                 sentence, got {answer:?}"
            );
            assert_eq!(
                answer.body["clusterName"],
                serde_json::Value::Null,
                "case {cluster:?}: {operation} must not echo the cluster it was asked \
                 for, got {answer:?}"
            );
        }

        // The parent resource is resolved FIRST, so a request that is also wrong in
        // another way still reports the unknown cluster rather than the other fault.
        let mut malformed = minimal_request("not a valid name", "KRO");
        malformed["configuration"] =
            serde_json::json!({ "ack": { "enableCrossNamespace": true } });
        let answer = create_capability(&router, cluster, &malformed).await;
        assert_eq!(
            (answer.status, code(&answer)),
            not_found,
            "case {cluster:?}: an unknown cluster wins over the other faults in the \
             request, got {answer:?}"
        );
    }

    // A cluster that exists but holds no capability by that name: the cluster is
    // found, the capability is not.
    for name in ["missing", "also_missing-1"] {
        let describe = describe_capability(&router, "real-cluster", name).await;
        assert_eq!(
            (describe.status, code(&describe)),
            (404, "ResourceNotFoundException".to_string()),
            "case {name:?}: DescribeCapability for an unknown capability, got {describe:?}"
        );
        let listed = list_capabilities(&router, "real-cluster", "").await;
        assert_eq!(
            listed_names(&listed),
            Vec::<String>::new(),
            "case {name:?}: a cluster with no capabilities lists none, got {listed:?}"
        );
    }
}

/// A capability lives inside a cluster, so the cluster leaving the store takes its
/// capabilities with it: a cluster re-created under the same name starts empty.
#[tokio::test]
async fn deleting_a_cluster_removes_the_capabilities_it_held() {
    let router = emulator();
    for case in ["recycled", "recycled_2"] {
        active_clusters(&router, &[case]).await;
        let created = create_capability(&router, case, &minimal_request("argocd", "ARGOCD")).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: setup CreateCapability, got {created:?}"
        );

        let deleted = call(
            &router,
            REGION,
            "DELETE",
            &format!("/clusters/{}", encode_label(case)),
            "",
        )
        .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: setup DeleteCluster, got {deleted:?}"
        );

        let gone = list_capabilities(&router, case, "").await;
        assert_eq!(
            gone.status, 404,
            "case {case:?}: a cluster that was deleted has nothing to list, got {gone:?}"
        );

        active_clusters(&router, &[case]).await;
        let listed = list_capabilities(&router, case, "").await;
        assert_eq!(
            listed_names(&listed),
            Vec::<String>::new(),
            "case {case:?}: a re-created cluster inherits no capabilities, got {listed:?}"
        );
        let describe = describe_capability(&router, case, "argocd").await;
        assert_eq!(
            describe.status, 404,
            "case {case:?}: the old cluster's capability is gone, got {describe:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: `DeleteCapability` removes the capability and answers with its final state;
// everything that reads it afterwards says it is gone, and the cluster is untouched.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delete_removes_the_capability_and_answers_with_its_final_state() {
    let router = emulator();
    // Rows differ along the type (so along which configuration is stored) and the
    // name shape.
    let cases: Vec<(&str, &str, &str)> = vec![
        ("a KRO capability", "del-kro", "KRO"),
        ("an ACK capability", "del-ack", "ACK"),
        ("an ARGOCD capability under a mixed-case name", "del-argo", "ARGOCD"),
    ];
    let clusters: Vec<&str> = cases.iter().map(|(_, cluster, _)| *cluster).collect();
    active_clusters(&router, &clusters).await;

    for (case, cluster, capability_type) in &cases {
        let name = if *capability_type == "ARGOCD" {
            "Argo_CD-1"
        } else {
            "capability"
        };
        let created = create_capability(&router, cluster, &minimal_request(name, capability_type)).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: setup CreateCapability, got {created:?}"
        );

        let deleted = delete_capability(&router, cluster, name).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected 200 from DeleteCapability, got {deleted:?}"
        );
        assert_eq!(
            capability(&deleted)["status"],
            serde_json::json!("DELETING"),
            "case {case:?}: a deleted capability reports DELETING, got {:?}",
            capability(&deleted)["status"]
        );
        for member in ["capabilityName", "arn", "type", "roleArn", "clusterName", "createdAt"] {
            assert_eq!(
                capability(&deleted)[member],
                capability(&created)[member],
                "case {case:?}: DeleteCapability must answer with the record it \
                 removed; {member} was {:?}, got {:?}",
                capability(&created)[member],
                capability(&deleted)[member]
            );
        }

        // Everything that reads it afterwards says it is gone.
        let describe = describe_capability(&router, cluster, name).await;
        assert_eq!(
            (describe.status, code(&describe)),
            (404, "ResourceNotFoundException".to_string()),
            "case {case:?}: DescribeCapability after a delete, got {describe:?}"
        );
        let again = delete_capability(&router, cluster, name).await;
        assert_eq!(
            (again.status, code(&again)),
            (404, "ResourceNotFoundException".to_string()),
            "case {case:?}: a second DeleteCapability, got {again:?}"
        );
        assert_eq!(
            listed_names(&list_capabilities(&router, cluster, "").await),
            Vec::<String>::new(),
            "case {case:?}: ListCapabilities no longer lists it"
        );
        // "Amazon EKS removes the capability infrastructure" — not the cluster.
        let cluster_after = describe_cluster(&router, cluster).await;
        assert_eq!(
            cluster_after.status, 200,
            "case {case:?}: the cluster outlives its capability, got {cluster_after:?}"
        );

        // And the name is free again: the same type can be created once more.
        let recreated = create_capability(&router, cluster, &minimal_request(name, capability_type)).await;
        assert_eq!(
            recreated.status, 200,
            "case {case:?}: the type and name are free after a delete, got {recreated:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a new capability reports CREATING until the clock owes it ACTIVE; a read
// only materialises the move that was already due.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_new_capability_reports_creating_until_the_clock_owes_it_active() {
    let router = emulator();
    active_clusters(&router, &["settling"]).await;

    // Rows: one capability per modelled type, all created before the span elapses.
    let created: Vec<(&str, String)> = ["ACK", "ARGOCD", "KRO"]
        .iter()
        .map(|capability_type| (*capability_type, capability_type.to_ascii_lowercase()))
        .collect();
    for (capability_type, name) in &created {
        let answer = create_capability(&router, "settling", &minimal_request(name, capability_type)).await;
        assert_eq!(
            answer.status, 200,
            "case {capability_type:?}: setup CreateCapability, got {answer:?}"
        );
        assert_eq!(
            capability(&answer)["status"],
            serde_json::json!("CREATING"),
            "case {capability_type:?}: a caller acting straight after the create still \
             sees CREATING, got {:?}",
            capability(&answer)["status"]
        );
        let described = describe_capability(&router, "settling", name).await;
        assert_eq!(
            capability(&described)["status"],
            serde_json::json!("CREATING"),
            "case {capability_type:?}: still CREATING before the span elapses, got {:?}",
            capability(&described)["status"]
        );
    }

    tokio::time::sleep(CAPABILITY_ACTIVATION_DELAY + Duration::from_millis(250)).await;

    for (capability_type, name) in &created {
        let described = describe_capability(&router, "settling", name).await;
        assert_eq!(
            capability(&described)["status"],
            serde_json::json!("ACTIVE"),
            "case {capability_type:?}: the clock owed it ACTIVE, got {:?}",
            capability(&described)["status"]
        );
    }
    let statuses: BTreeSet<String> = list_capabilities(&router, "settling", "")
        .await
        .body["capabilities"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["status"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        statuses,
        BTreeSet::from(["ACTIVE".to_string()]),
        "ListCapabilities reports the same status DescribeCapability does, got {statuses:?}"
    );
}

/// A capability is provisioned for a live control plane, so a cluster that is not
/// `ACTIVE` cannot take one: that is what `InvalidRequestException` ("invalid given
/// the state of the cluster") reports.
#[tokio::test]
async fn a_cluster_that_is_not_active_yet_cannot_take_a_capability() {
    let router = emulator();
    // Rows differ along the cluster name, created and then used immediately — within
    // the CREATING span the clock owes the cluster.
    for cluster in ["too-soon", "Too_Soon-2"] {
        let created = call(
            &router,
            REGION,
            "POST",
            "/clusters",
            &serde_json::json!({
                "name": cluster,
                "roleArn": "arn:aws:iam::000000000000:role/eksClusterRole",
                "resourcesVpcConfig": { "subnetIds": ["subnet-11111111", "subnet-22222222"] },
            })
            .to_string(),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {cluster:?}: setup CreateCluster, got {created:?}"
        );

        let answer = create_capability(&router, cluster, &minimal_request("early", "KRO")).await;
        assert_eq!(
            (answer.status, code(&answer)),
            (400, "InvalidRequestException".to_string()),
            "case {cluster:?}: a CREATING cluster cannot take a capability, got {answer:?}"
        );

        tokio::time::sleep(CLUSTER_ACTIVATION_DELAY + Duration::from_millis(250)).await;
        let later = create_capability(&router, cluster, &minimal_request("early", "KRO")).await;
        assert_eq!(
            later.status, 200,
            "case {cluster:?}: once the cluster is ACTIVE the same request succeeds, \
             got {later:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: paging returns every capability of the cluster exactly once, in name order,
// at any page size the model allows; `nextToken` appears only when more is left.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn paging_at_any_size_yields_every_capability_exactly_once() {
    let router = emulator();
    active_clusters(&router, &["paged"]).await;
    // One capability per modelled type is all a cluster may hold, so the names are
    // the types' own.
    let mut stored: Vec<String> = Vec::new();
    for capability_type in ["ACK", "ARGOCD", "KRO"] {
        let name = capability_type.to_ascii_lowercase();
        let created = create_capability(&router, "paged", &minimal_request(&name, capability_type)).await;
        assert_eq!(
            created.status, 200,
            "setup: CreateCapability {name:?}, got {created:?}"
        );
        stored.push(name);
    }
    stored.sort();

    // Page sizes: each value from 1 up to one more than the number stored, plus the
    // published maximum of the modelled 1..100 `maxResults` range and the absent
    // case.
    let sizes: Vec<Option<usize>> = (1..=stored.len() + 1)
        .map(Some)
        .chain([Some(100), None])
        .collect();
    for size in &sizes {
        let case = format!("maxResults={size:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query = Vec::new();
            if let Some(size) = size {
                query.push(format!("maxResults={size}"));
            }
            if let Some(token) = &token {
                query.push(format!("nextToken={token}"));
            }
            let answer = list_capabilities(&router, "paged", &query.join("&")).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200 from ListCapabilities, got {answer:?}"
            );
            let page = listed_names(&answer);
            if let Some(size) = size {
                assert!(
                    page.len() <= *size,
                    "case {case:?}: a page holds at most {size} summaries, got {page:?}"
                );
            }
            seen.extend(page);
            pages += 1;
            token = answer.body["nextToken"].as_str().map(str::to_owned);
            if token.is_none() {
                break;
            }
            assert!(
                pages <= stored.len() + 2,
                "case {case:?}: paging must terminate, saw {pages} pages"
            );
        }
        assert_eq!(
            seen, stored,
            "case {case:?}: every capability exactly once, in name order; got {seen:?}"
        );
    }

    // A token this service did not mint is a bad parameter, not an empty page.
    for (case, token) in [
        ("not hex", "zzzz"),
        ("hex but too short", "00"),
        ("hex with a bad provenance tag", "00000000617267"),
    ] {
        let answer = list_capabilities(&router, "paged", &format!("nextToken={token}")).await;
        assert_eq!(
            (answer.status, code(&answer)),
            (400, "InvalidParameterException".to_string()),
            "case {case:?} ({token:?}): expected a refused token, got {answer:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: `clientRequestToken` is `@idempotencyToken` — a retry carrying the token of
// the call that created the capability is answered with that capability; any other
// request for a name already taken conflicts.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_retry_with_the_same_idempotency_token_is_absorbed() {
    // Rows differ along the relationship between the two requests' tokens: equal,
    // different, present-then-absent, absent-then-present, absent on both, and blank
    // on both (a blank token identifies nothing, so it is not a retry marker).
    /// One row: the case name, its cluster, the first call's token, the second
    /// call's token, and the status the second call must answer with.
    type Case = (
        &'static str,
        &'static str,
        Option<&'static str>,
        Option<&'static str>,
        u16,
    );
    let cases: Vec<Case> = vec![
        ("the same token twice", "idem-1", Some("token-a"), Some("token-a"), 200),
        ("two different tokens", "idem-2", Some("token-a"), Some("token-b"), 409),
        ("a token, then none", "idem-3", Some("token-a"), None, 409),
        ("none, then a token", "idem-4", None, Some("token-a"), 409),
        ("no token on either call", "idem-5", None, None, 409),
        ("a blank token on both calls", "idem-6", Some("   "), Some("   "), 409),
    ];

    let router = emulator();
    let clusters: Vec<&str> = cases.iter().map(|(_, cluster, ..)| *cluster).collect();
    active_clusters(&router, &clusters).await;

    for (case, cluster, first_token, second_token, expected_status) in &cases {
        let request = |token: Option<&str>| {
            let mut body = minimal_request("idempotent", "ARGOCD");
            if let Some(token) = token {
                body["clientRequestToken"] = serde_json::json!(token);
            }
            body
        };

        let first = create_capability(&router, cluster, &request(*first_token)).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the first create succeeds, got {first:?}"
        );
        let second = create_capability(&router, cluster, &request(*second_token)).await;
        assert_eq!(
            second.status, *expected_status,
            "case {case:?}: expected HTTP {expected_status} from the second create, \
             got {second:?}"
        );
        if *expected_status == 200 {
            assert_eq!(
                capability(&second)["arn"],
                capability(&first)["arn"],
                "case {case:?}: an absorbed retry answers with the capability the \
                 first call created, got {:?}",
                capability(&second)["arn"]
            );
            assert_eq!(
                capability(&second)["createdAt"],
                capability(&first)["createdAt"],
                "case {case:?}: an absorbed retry does not re-create the capability, \
                 got {:?}",
                capability(&second)["createdAt"]
            );
        } else {
            assert_eq!(
                code(&second),
                "ResourceInUseException",
                "case {case:?}: expected ResourceInUseException, got {second:?}"
            );
        }
        // Either way the cluster holds exactly one capability.
        let listed = listed_names(&list_capabilities(&router, cluster, "").await);
        assert_eq!(
            listed,
            vec!["idempotent".to_string()],
            "case {case:?}: exactly one capability is stored, got {listed:?}"
        );
    }
}
