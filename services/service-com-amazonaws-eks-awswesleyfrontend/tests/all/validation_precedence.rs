// SPDX-License-Identifier: CC0-1.0

//! What this service answers BEFORE a refusal of the request body: the pass-role
//! authorization of a role the request hands Amazon EKS, and the resolution of the
//! cluster the request is addressed to.
//!
//! The rule is one ORDER, so each suite asserts it over a TABLE whose rows differ
//! along what the order generalises over: which operation is called, which modelled
//! constraint the body breaks (an enum member, an integer range, a map's entry
//! count), which member carries the role, and whether the request would have been
//! served at all. Each assertion names its case and prints the actual value.
//!
//! The wire harness (`Answer`, `emulator`, `call`, `encode_label`) is the shared one
//! in `tests/all/wire.rs`.

use std::time::Duration;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    cluster_not_found_message, max_results_range_message, CLUSTER_ACTIVATION_DELAY,
    CROSS_ACCOUNT_PASS_ROLE_REFUSAL, MAX_LIST_MAX_RESULTS, MIN_LIST_MAX_RESULTS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, encode_label, error_code, Answer};

const REGION: &str = "us-west-2";

/// A role of the one account this emulator serves, which this caller may pass.
const OWN_ROLE: &str = "arn:aws:iam::000000000000:role/eksRole";

/// The cluster name no suite here creates.
const GHOST: &str = "sim2real-ghost-cluster";

/// A name that breaks `CreateCluster$name`'s modelled pattern
/// (`^[0-9A-Za-z][A-Za-z0-9\-_]*$`), so the framework's walk refuses the body.
const BAD_CLUSTER_NAME: &str = "sim2real behav bad name!";

async fn active_cluster(router: &MultiServiceRouter, name: &str) {
    let body = serde_json::json!({
        "name": name,
        "roleArn": OWN_ROLE,
        "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"] },
    });
    let created = call(router, REGION, "POST", "/clusters", &body.to_string()).await;
    assert_eq!(
        created.status, 200,
        "setup: expected 200 from CreateCluster for {name:?}, got {created:?}"
    );
    tokio::time::sleep(CLUSTER_ACTIVATION_DELAY + Duration::from_millis(250)).await;
}

/// One request a row sends: the HTTP method, the URI it is sent to and its body.
struct Request {
    method: &'static str,
    uri: String,
    body: serde_json::Value,
}

impl Request {
    fn post(uri: String, body: serde_json::Value) -> Self {
        Self {
            method: "POST",
            uri,
            body,
        }
    }

    fn get(uri: String) -> Self {
        Self {
            method: "GET",
            uri,
            body: serde_json::Value::Null,
        }
    }

    async fn send(&self, router: &MultiServiceRouter) -> Answer {
        let body = match &self.body {
            serde_json::Value::Null => String::new(),
            value => value.to_string(),
        };
        call(router, REGION, self.method, &self.uri, &body).await
    }
}

fn cluster_path(cluster: &str, suffix: &str) -> String {
    format!("/clusters/{}{suffix}", encode_label(cluster))
}

/// How one row of the table below builds its request, given the cluster name it is
/// addressed to: the SAME request is sent to a cluster that exists and to one that
/// does not, so the two legs differ in nothing but that name.
type BuildRequest = fn(&str) -> Request;

// ---------------------------------------------------------------------------
// RULE: a request addressed to a cluster that does not exist is answered with that
// missing cluster, whatever is wrong with its BODY — while what is wrong with the
// request's own addressing (the page size it asks for, in the query string) is
// answered before the cluster is resolved at all.
// ---------------------------------------------------------------------------

/// The table spans what the rule generalises over: which operation the request was
/// sent to (a configuration update, three child creates and an association), and
/// which kind of modelled constraint its body breaks — an enum member, an integer
/// range below its minimum, and a map below its minimum entry count.
#[tokio::test]
async fn an_unknown_cluster_is_reported_before_the_body_is_refused() {
    let router = emulator();
    // A cluster that DOES exist, so each row's body refusal is still reachable: the
    // same request against it must be refused as the bad body it is.
    active_cluster(&router, "precedence-cluster").await;

    let rows: [(&str, BuildRequest); 6] = [
        (
            "UpdateClusterConfig with a log type outside the modelled enum",
            |cluster| {
                Request::post(
                    cluster_path(cluster, "/update-config"),
                    serde_json::json!({
                        "logging": {
                            "clusterLogging": [
                                { "types": ["definitely-not-a-log-type"], "enabled": true },
                            ],
                        },
                    }),
                )
            },
        ),
        (
            "CreateNodegroup with a maxSize below its modelled minimum",
            |cluster| {
                Request::post(
                    cluster_path(cluster, "/node-groups"),
                    serde_json::json!({
                        "nodegroupName": "ng",
                        "nodeRole": OWN_ROLE,
                        "subnets": ["subnet-aaaa1111"],
                        "scalingConfig": { "minSize": 0, "maxSize": 0, "desiredSize": 0 },
                    }),
                )
            },
        ),
        (
            "CreateAddon with an empty tag map, below the modelled entry count",
            |cluster| {
                Request::post(
                    cluster_path(cluster, "/addons"),
                    serde_json::json!({ "addonName": "vpc-cni", "tags": {} }),
                )
            },
        ),
        (
            "CreateAccessEntry with an empty tag map",
            |cluster| {
                Request::post(
                    cluster_path(cluster, "/access-entries"),
                    serde_json::json!({
                        "principalArn": "arn:aws:iam::000000000000:role/admin",
                        "tags": {},
                    }),
                )
            },
        ),
        (
            "AssociateIdentityProviderConfig with an empty tag map",
            |cluster| {
                Request::post(
                    cluster_path(cluster, "/identity-provider-configs/associate"),
                    serde_json::json!({
                        "oidc": {
                            "identityProviderConfigName": "provider",
                            "issuerUrl": "https://oidc.example.com",
                            "clientId": "kubernetes",
                        },
                        "tags": {},
                    }),
                )
            },
        ),
        (
            "CreatePodIdentityAssociation with an empty tag map",
            |cluster| {
                Request::post(
                    cluster_path(cluster, "/pod-identity-associations"),
                    serde_json::json!({
                        "namespace": "default",
                        "serviceAccount": "sa",
                        "roleArn": OWN_ROLE,
                        "tags": {},
                    }),
                )
            },
        ),
    ];

    for (case, build) in &rows {
        // On the cluster that does not exist: the missing cluster, named.
        let answer = build(GHOST).send(&router).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?} on an unknown cluster: expected HTTP 404, got {answer:?}"
        );
        let code = error_code(&answer);
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case:?} on an unknown cluster: expected ResourceNotFoundException, \
             got {code:?}"
        );
        let expected = cluster_not_found_message(GHOST);
        assert_eq!(
            answer.body["message"],
            serde_json::json!(expected),
            "case {case:?} on an unknown cluster: expected message {expected:?}, got {:?}",
            answer.body["message"]
        );

        // On the cluster that DOES exist, the same body is refused as the bad body it
        // is: this layer restores an order, it does not swallow a refusal.
        let refused = build("precedence-cluster").send(&router).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?} on an existing cluster: expected HTTP 400, got {refused:?}"
        );
        let code = error_code(&refused);
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?} on an existing cluster: expected InvalidParameterException, \
             got {code:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: the page size a listing asks for travels in the QUERY STRING, so it is part
// of the request's own addressing: a value outside the bounds the model publishes is
// refused 400 `InvalidParameterException` with the page-size sentence BEFORE the
// cluster the listing is addressed to is resolved, and the refusal names no resource.
// ---------------------------------------------------------------------------

/// The table spans what the rule generalises over: which listing of a cluster's
/// resources was called (four, including one whose label member the model calls
/// `name` rather than `clusterName`), which published bound the value broke, and
/// whether the cluster it is addressed to exists at all.
///
/// Source for the bounds: every `maxResults` of this API is `@range {min: 1, max:
/// 100}` (`smithy_query query="operation" operation="ListAddons"` reports
/// `ListAddonsRequestMaxResults` with those bounds), read here through the published
/// constants rather than written out.
#[tokio::test]
async fn a_page_size_outside_its_bounds_is_refused_before_the_cluster() {
    let router = emulator();
    active_cluster(&router, "page-size-cluster").await;

    let listings = [
        ("ListNodegroups", "/node-groups"),
        ("ListAddons", "/addons"),
        ("ListCapabilities", "/capabilities"),
        ("ListUpdates", "/updates"),
    ];
    // One value below the published minimum, one above the published maximum, and
    // both published bounds themselves, which are inside the range.
    let refused = [MIN_LIST_MAX_RESULTS - 1, MAX_LIST_MAX_RESULTS + 1];
    let accepted = [MIN_LIST_MAX_RESULTS, MAX_LIST_MAX_RESULTS];

    for (operation, suffix) in listings {
        for size in refused {
            for cluster in [GHOST, "page-size-cluster"] {
                let case = format!("{operation} maxResults={size} on cluster {cluster:?}");
                let answer =
                    Request::get(cluster_path(cluster, &format!("{suffix}?maxResults={size}")))
                        .send(&router)
                        .await;
                assert_eq!(
                    answer.status, 400,
                    "case {case}: expected HTTP 400, got {answer:?}"
                );
                let code = error_code(&answer);
                assert_eq!(
                    code, "InvalidParameterException",
                    "case {case}: expected InvalidParameterException, got {code:?}"
                );
                let expected = max_results_range_message();
                assert_eq!(
                    answer.body["message"],
                    serde_json::json!(expected),
                    "case {case}: expected message {expected:?}, got {:?}",
                    answer.body["message"]
                );
                assert!(
                    answer.body.get("clusterName").is_none(),
                    "case {case}: the page-size refusal names no resource, got {:?}",
                    answer.body["clusterName"]
                );
            }
        }
        // A page size INSIDE the bounds leaves the order as it was: the listing is
        // served on the cluster that exists and reports the missing one on the ghost.
        for size in accepted {
            let served =
                Request::get(cluster_path("page-size-cluster", &format!("{suffix}?maxResults={size}")))
                    .send(&router)
                    .await;
            assert_eq!(
                served.status, 200,
                "case {operation} maxResults={size} on an existing cluster: expected 200, \
                 got {served:?}"
            );
            let missing = Request::get(cluster_path(GHOST, &format!("{suffix}?maxResults={size}")))
                .send(&router)
                .await;
            assert_eq!(
                missing.status, 404,
                "case {operation} maxResults={size} on an unknown cluster: expected 404, \
                 got {missing:?}"
            );
        }
    }
}

/// A well-formed body is served exactly as before — the ordering layer claims
/// nothing when there is no body refusal to pre-empt.
#[tokio::test]
async fn a_well_formed_request_is_unaffected() {
    let router = emulator();
    active_cluster(&router, "unaffected-cluster").await;

    let served = call(
        &router,
        REGION,
        "GET",
        &cluster_path("unaffected-cluster", "/node-groups?maxResults=1"),
        "",
    )
    .await;
    assert_eq!(
        served.status, 200,
        "case a well-formed listing on an existing cluster: expected 200, got {served:?}"
    );

    let not_found = call(
        &router,
        REGION,
        "GET",
        &cluster_path(GHOST, "/node-groups?maxResults=1"),
        "",
    )
    .await;
    assert_eq!(
        not_found.status, 404,
        "case a well-formed listing on an unknown cluster: expected 404, got {not_found:?}"
    );
    let expected = cluster_not_found_message(GHOST);
    assert_eq!(
        not_found.body["message"],
        serde_json::json!(expected),
        "case a well-formed listing on an unknown cluster: expected message {expected:?}, \
         got {:?}",
        not_found.body["message"]
    );
}

// ---------------------------------------------------------------------------
// RULE: a role the caller may not have Amazon EKS pass is refused before the body is
// judged — the authorization is decided from the value alone, so nothing about the
// rest of the request can earn or avoid it.
// ---------------------------------------------------------------------------

/// The table spans what the rule generalises over: which operation carries the role
/// and in which member, how the value fails to name a role of this account (another
/// account, not an ARN at all, an ARN of another service), and what else is wrong
/// with the request (a pattern-breaking name, a range-breaking size, an empty tag
/// map, a cluster that does not exist).
#[tokio::test]
async fn a_role_this_caller_cannot_pass_is_refused_before_the_body() {
    let router = emulator();
    active_cluster(&router, "pass-role-cluster").await;

    let unpassable = [
        "arn:aws:iam::111122223333:role/elsewhere",
        "not-an-arn",
        "arn:aws:s3:::a-bucket",
    ];

    for role in unpassable {
        let rows: [(String, Request); 5] = [
            (
                format!("CreateCluster with roleArn {role:?} and a name breaking its pattern"),
                Request::post(
                    "/clusters".to_string(),
                    serde_json::json!({
                        "name": BAD_CLUSTER_NAME,
                        "roleArn": role,
                        "resourcesVpcConfig": { "subnetIds": ["subnet-nope"] },
                    }),
                ),
            ),
            (
                format!(
                    "CreateCluster with computeConfig.nodeRoleArn {role:?} and a bad name"
                ),
                Request::post(
                    "/clusters".to_string(),
                    serde_json::json!({
                        "name": BAD_CLUSTER_NAME,
                        "roleArn": OWN_ROLE,
                        "resourcesVpcConfig": { "subnetIds": ["subnet-nope"] },
                        "computeConfig": { "enabled": true, "nodeRoleArn": role },
                    }),
                ),
            ),
            (
                format!(
                    "CreateNodegroup with nodeRole {role:?}, a bad scalingConfig and an \
                     unknown cluster"
                ),
                Request::post(
                    cluster_path(GHOST, "/node-groups"),
                    serde_json::json!({
                        "nodegroupName": "ng",
                        "nodeRole": role,
                        "subnets": ["subnet-aaaa1111"],
                        "scalingConfig": { "minSize": 0, "maxSize": 0, "desiredSize": 0 },
                    }),
                ),
            ),
            (
                format!("CreateCapability with roleArn {role:?} and an empty tag map"),
                Request::post(
                    cluster_path("pass-role-cluster", "/capabilities"),
                    serde_json::json!({
                        "capabilityName": "cap",
                        "type": "ACK",
                        "roleArn": role,
                        "deletePropagationPolicy": "RETAIN",
                        "tags": {},
                    }),
                ),
            ),
            (
                format!(
                    "CreateFargateProfile with podExecutionRoleArn {role:?} and an empty \
                     tag map"
                ),
                Request::post(
                    cluster_path("pass-role-cluster", "/fargate-profiles"),
                    serde_json::json!({
                        "fargateProfileName": "profile",
                        "podExecutionRoleArn": role,
                        "tags": {},
                    }),
                ),
            ),
        ];

        for (case, request) in &rows {
            let answer = request.send(&router).await;
            assert_eq!(
                answer.status, 403,
                "case {case:?}: expected HTTP 403, got {answer:?}"
            );
            let code = error_code(&answer);
            assert_eq!(
                code, "AccessDeniedException",
                "case {case:?}: expected AccessDeniedException, got {code:?}"
            );
            assert_eq!(
                answer.body["message"],
                serde_json::json!(CROSS_ACCOUNT_PASS_ROLE_REFUSAL),
                "case {case:?}: expected the cross-account pass-role sentence, got {:?}",
                answer.body["message"]
            );
        }
    }

    // The positive control: the SAME malformed bodies with a role this caller may
    // pass earn the body refusal, so the denial above is earned by the role and not
    // by the body.
    let controls: [(&str, Request); 2] = [
        (
            "CreateCluster with an in-account role and a name breaking its pattern",
            Request::post(
                "/clusters".to_string(),
                serde_json::json!({
                    "name": BAD_CLUSTER_NAME,
                    "roleArn": OWN_ROLE,
                    "resourcesVpcConfig": { "subnetIds": ["subnet-nope"] },
                }),
            ),
        ),
        (
            "CreateCapability with an in-account role and an empty tag map",
            Request::post(
                cluster_path("pass-role-cluster", "/capabilities"),
                serde_json::json!({
                    "capabilityName": "cap",
                    "type": "ACK",
                    "roleArn": OWN_ROLE,
                    "deletePropagationPolicy": "RETAIN",
                    "tags": {},
                }),
            ),
        ),
    ];
    for (case, request) in &controls {
        let answer = request.send(&router).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        let code = error_code(&answer);
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {code:?}"
        );
    }
}
