// SPDX-License-Identifier: CC0-1.0

//! The ORDER the EKS add-on surface judges a request in, and the error CLASS each
//! stage answers with.
//!
//! RULE: Amazon EKS judges a request in three stages — first the request's own
//! ADDRESSING and AUTHORIZATION (the `@httpLabel` cluster name, the role it asks
//! Amazon EKS to pass, the page size it asks for in the query string), then the
//! EXISTENCE of the cluster the request is addressed to, and only then the rest of
//! the body and the state of the resources it names. A request with several problems
//! is refused for the EARLIEST stage one of its problems sits in, so a caller is
//! always told the thing it has to fix first.
//!
//! Each stage is asserted over a TABLE whose rows differ along what the rule
//! generalises over — which add-on operation was called, how the value misses its
//! rule, and what ELSE is wrong with the same request — rather than over one recorded
//! request. The page-size stage of the same rule, which spans every listing of a
//! cluster's resources rather than the add-on surface alone, is asserted in
//! `tests/all/validation_precedence.rs`.
//!
//! The wire harness (`Answer`, `emulator`, `call`, `encode_label`) is the shared one
//! in `tests/all/wire.rs`.

use std::time::Duration;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    cluster_name_refusal, cluster_not_found_message, ADDON_ACTIVATION_DELAY,
    ADDON_SERVICE_ACCOUNT_ROLE_ARN_REFUSAL, CLUSTER_ACTIVATION_DELAY,
    CROSS_ACCOUNT_PASS_ROLE_REFUSAL,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{
    call, emulator, emulator_with_identity, encode_label, error_code, query, Answer,
};

/// The region every request in this suite is signed for.
const REGION: &str = "us-west-2";

/// A role of the one account this emulator serves, which this caller may pass.
const OWN_ROLE: &str = "arn:aws:iam::000000000000:role/AddonRole";

/// An add-on every catalog build set publishes, so a row that is meant to be refused
/// for something else is never refused for its add-on name.
const ADDON: &str = "vpc-cni";

/// The cluster name no suite here creates: every stage ahead of the cluster has to
/// answer even when the cluster is absent, which is the point of the order.
const GHOST: &str = "sim2real-ghost-cluster";

/// The cluster this suite creates, so each stage can also be shown on a cluster that
/// DOES exist.
const CLUSTER: &str = "addon-precedence-cluster";

fn cluster_path(cluster: &str, suffix: &str) -> String {
    format!("/clusters/{}{suffix}", encode_label(cluster))
}

/// One cluster in [`REGION`], waited into `ACTIVE` so the add-on layers that need a
/// running control plane are past.
async fn active_cluster(router: &MultiServiceRouter, name: &str) {
    let body = serde_json::json!({
        "name": name,
        "roleArn": OWN_ROLE,
        "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"] },
    });
    let created = call(router, REGION, "POST", "/clusters", &body.to_string()).await;
    assert_eq!(
        created.status, 200,
        "setup CreateCluster {name:?}: got {created:?}"
    );
    tokio::time::sleep(CLUSTER_ACTIVATION_DELAY + Duration::from_millis(250)).await;
}

/// One installed add-on, settled, so `UpdateAddon` rows reach their own stages.
async fn active_addon(router: &MultiServiceRouter, cluster: &str, addon: &str) {
    let created = call(
        router,
        REGION,
        "POST",
        &cluster_path(cluster, "/addons"),
        &serde_json::json!({ "addonName": addon }).to_string(),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "setup CreateAddon {addon:?} on {cluster:?}: got {created:?}"
    );
    tokio::time::sleep(ADDON_ACTIVATION_DELAY + Duration::from_millis(250)).await;
}

/// One request a row sends: the method, the URI suffix under the cluster label, and
/// the body.
struct AddonCall {
    operation: &'static str,
    method: &'static str,
    suffix: String,
    body: serde_json::Value,
}

impl AddonCall {
    /// The five add-on operations addressed to a cluster by `@httpLabel`: the install,
    /// the update, the two reads and the delete.
    fn every_operation(addon: &str) -> Vec<Self> {
        vec![
            Self {
                operation: "CreateAddon",
                method: "POST",
                suffix: "/addons".to_string(),
                body: serde_json::json!({ "addonName": addon }),
            },
            Self {
                operation: "UpdateAddon",
                method: "POST",
                suffix: format!("/addons/{}/update", encode_label(addon)),
                body: serde_json::json!({ "addonVersion": "v1.0.0-eksbuild.1" }),
            },
            Self {
                operation: "ListAddons",
                method: "GET",
                suffix: "/addons".to_string(),
                body: serde_json::Value::Null,
            },
            Self {
                operation: "DescribeAddon",
                method: "GET",
                suffix: format!("/addons/{}", encode_label(addon)),
                body: serde_json::Value::Null,
            },
            Self {
                operation: "DeleteAddon",
                method: "DELETE",
                suffix: format!("/addons/{}", encode_label(addon)),
                body: serde_json::Value::Null,
            },
        ]
    }

    async fn send(&self, router: &MultiServiceRouter, cluster: &str) -> Answer {
        let body = match &self.body {
            serde_json::Value::Null => String::new(),
            value => value.to_string(),
        };
        call(
            router,
            REGION,
            self.method,
            &cluster_path(cluster, &self.suffix),
            &body,
        )
        .await
    }
}

// ---------------------------------------------------------------------------
// STAGE 1a: the `@httpLabel` cluster name. Every cluster name of this API obeys one
// grammar and one length maximum (`ClusterName`: `@length(1..100)` + `@pattern
// ^[0-9A-Za-z][A-Za-z0-9\-_]*$`), so a label outside it names no cluster in ANY
// Region: it is refused 400 `InvalidParameterException` with the sentence Amazon EKS
// states for that parameter, reporting the offending value, rather than looked up.
// ---------------------------------------------------------------------------

/// The table spans what the rule generalises over: which add-on operation was called,
/// and HOW the label misses the grammar — a cluster ARN passed where a bare name
/// belongs, a space and a punctuation character, a leading dash, a path separator,
/// and a name one character past the published maximum (the length case, which earns
/// that parameter's own length sentence).
///
/// Source for the grammar and the maximum: `smithy_query query="operation"
/// operation="CreateAddon"` reports `clusterName` -> `ClusterName`, `httpLabel`,
/// length 1..100, pattern `^[0-9A-Za-z][A-Za-z0-9\-_]*$`. The expected sentence is
/// read from the service's own published wording rather than written out here.
#[tokio::test]
async fn a_cluster_label_outside_the_grammar_is_refused_before_anything_is_looked_up() {
    let router = emulator();

    let labels = [
        (
            "a cluster ARN where a bare name belongs",
            "arn:aws:eks:us-west-2:000000000000:cluster/addon-precedence".to_string(),
        ),
        ("a space and a bang", "sim2real probe bad!".to_string()),
        ("a leading dash", "-leading-dash".to_string()),
        ("a path separator", "cluster/child".to_string()),
        ("the empty-ish dot", ".".to_string()),
        ("one character past the maximum", "c".repeat(101)),
    ];

    for (how, label) in &labels {
        for request in AddonCall::every_operation(ADDON) {
            let case = format!("{} with {how}", request.operation);
            let answer = request.send(&router, label).await;
            assert_eq!(
                answer.status, 400,
                "case {case}: expected HTTP 400, got {answer:?}"
            );
            let code = error_code(&answer);
            assert_eq!(
                code, "InvalidParameterException",
                "case {case}: expected InvalidParameterException, got {code:?}"
            );
            let expected = cluster_name_refusal("clusterName", label);
            assert_eq!(
                answer.body["message"],
                serde_json::json!(expected),
                "case {case}: expected message {expected:?}, got {:?}",
                answer.body["message"]
            );
            assert_eq!(
                answer.body["clusterName"],
                serde_json::json!(label),
                "case {case}: the refusal reports the offending value, got {:?}",
                answer.body["clusterName"]
            );
        }
    }

    // The same refusal when the BODY is wrong too, including the one way a body can be
    // wrong that the request cannot even be deserialized from: `CreateAddon$addonName`
    // is required, and a request that omits it is refused by the codec. The label
    // still decides, because it is judged first.
    let bodies = [
        (
            "no addonName at all",
            serde_json::json!({}),
        ),
        (
            "an addonName no add-on has and an empty tag map",
            serde_json::json!({ "addonName": "not an addon name", "tags": {} }),
        ),
        (
            "a resolveConflicts outside the enum",
            serde_json::json!({ "addonName": ADDON, "resolveConflicts": "BANANA" }),
        ),
    ];
    let label = "arn:aws:eks:us-west-2:000000000000:cluster/addon-precedence";
    for (how, body) in &bodies {
        let answer = call(
            &router,
            REGION,
            "POST",
            &cluster_path(label, "/addons"),
            &body.to_string(),
        )
        .await;
        let case = format!("CreateAddon with a bad label and {how}");
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400, got {answer:?}"
        );
        let expected = cluster_name_refusal("clusterName", label);
        assert_eq!(
            answer.body["message"],
            serde_json::json!(expected),
            "case {case}: expected message {expected:?}, got {:?}",
            answer.body["message"]
        );
        assert_eq!(
            answer.body["clusterName"],
            serde_json::json!(label),
            "case {case}: the refusal reports the offending value, got {:?}",
            answer.body["clusterName"]
        );
    }

    // The control: a label INSIDE the grammar naming a cluster that does not exist is
    // the missing cluster, not a parameter complaint — the grammar check refuses a
    // value no cluster could carry, it does not stand in for the lookup.
    for request in AddonCall::every_operation(ADDON) {
        let answer = request.send(&router, GHOST).await;
        assert_eq!(
            answer.status, 404,
            "case {} on a well-formed name no cluster carries: expected HTTP 404, got \
             {answer:?}",
            request.operation
        );
        let code = error_code(&answer);
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {} on a well-formed name no cluster carries: expected \
             ResourceNotFoundException, got {code:?}",
            request.operation
        );
    }
}

// ---------------------------------------------------------------------------
// STAGE 1b: the role the request asks Amazon EKS to pass. `serviceAccountRoleArn` is
// the IAM role the add-on's Kubernetes service account is given, so the add-on
// operations ask two questions about it before the cluster is resolved: whether this
// caller may pass it at all (403 `AccessDeniedException`, the pass-role rule every
// role-bearing member of this service obeys — the `pass_role_authorization` suite owns
// the whole value table), and whether IAM holds the role it names (400
// `InvalidParameterException` with the sentence this API states for that parameter).
// ---------------------------------------------------------------------------

/// The table spans what the rule generalises over: which add-on operation carries the
/// role, HOW the value fails to name a role this caller can pass, and whether the
/// cluster the request is addressed to exists at all — because the role is judged
/// before that is asked.
#[tokio::test]
async fn a_role_this_caller_cannot_pass_is_refused_before_the_cluster() {
    let router = emulator();
    active_cluster(&router, CLUSTER).await;
    active_addon(&router, CLUSTER, ADDON).await;

    let unpassable = [
        ("a role of another account", "arn:aws:iam::111122223333:role/elsewhere"),
        ("not an ARN at all", "AddonRole"),
        (
            "another service's ARN",
            "arn:aws:eks:us-west-2:000000000000:cluster/addon-precedence-cluster",
        ),
        ("an account that is not an account id", "arn:aws:iam::12345:role/short"),
        ("the empty string", ""),
    ];

    for (how, role) in unpassable {
        for (operation, method, suffix) in [
            ("CreateAddon", "POST", "/addons".to_string()),
            (
                "UpdateAddon",
                "POST",
                format!("/addons/{}/update", encode_label(ADDON)),
            ),
        ] {
            for cluster in [GHOST, CLUSTER] {
                let case = format!("{operation} with {how} on cluster {cluster:?}");
                let body = serde_json::json!({
                    "addonName": ADDON,
                    "serviceAccountRoleArn": role,
                });
                let answer = call(
                    &router,
                    REGION,
                    method,
                    &cluster_path(cluster, &suffix),
                    &body.to_string(),
                )
                .await;
                assert_eq!(
                    answer.status, 403,
                    "case {case}: expected HTTP 403, got {answer:?}"
                );
                let actual = error_code(&answer);
                assert_eq!(
                    actual, "AccessDeniedException",
                    "case {case}: expected AccessDeniedException, got {actual:?}"
                );
                assert_eq!(
                    answer.body["message"],
                    serde_json::json!(CROSS_ACCOUNT_PASS_ROLE_REFUSAL),
                    "case {case}: expected the cross-account sentence, got {:?}",
                    answer.body["message"]
                );
                assert!(
                    answer.body.get("clusterName").is_none(),
                    "case {case}: the pass-role denial names no resource, got {:?}",
                    answer.body["clusterName"]
                );
            }
        }
    }

    // The control: a role of THIS account is passed, so the request reaches its own
    // stages — the missing cluster on the ghost, and the add-on itself on the cluster
    // that exists. This emulator has no IAM mounted beside it here, so the question
    // IAM answers about the role is not asked at all (see the suite below).
    let body = serde_json::json!({ "addonName": "coredns", "serviceAccountRoleArn": OWN_ROLE });
    let missing = call(
        &router,
        REGION,
        "POST",
        &cluster_path(GHOST, "/addons"),
        &body.to_string(),
    )
    .await;
    assert_eq!(
        missing.status, 404,
        "case CreateAddon with an in-account role on an unknown cluster: expected 404, \
         got {missing:?}"
    );
    let served = call(
        &router,
        REGION,
        "POST",
        &cluster_path(CLUSTER, "/addons"),
        &body.to_string(),
    )
    .await;
    assert_eq!(
        served.status, 200,
        "case CreateAddon with an in-account role on an existing cluster: expected 200, \
         got {served:?}"
    );
}

/// The ROLE ITSELF: a value that names no role IAM could hold — a role name or path
/// outside the grammar IAM publishes for one — cannot be given to the add-on's service
/// account, and that is refused before the cluster is resolved.
///
/// The rows differ along which add-on operation carries the role and HOW the value
/// misses the grammar (a space, a character outside the name set, a name past its
/// published maximum, an empty path segment), and along whether the cluster the request
/// is addressed to exists. The positive control is the same request naming a role of
/// this account that IS inside the grammar, including one under a path.
///
/// Source for the grammar: IAM publishes `roleNameType` as `@length(1..64)` +
/// `@pattern ^[\w+=,.@-]+$` and `pathType` as printable ASCII between slashes
/// (`smithy_query query="shape" shape="roleNameType,pathType"` on the IAM model).
#[tokio::test]
async fn a_role_arn_no_iam_role_could_carry_is_refused_before_the_cluster() {
    let router = emulator();
    active_cluster(&router, CLUSTER).await;
    active_addon(&router, CLUSTER, ADDON).await;

    let account = engine::emulator_account_id();
    let refused = [
        ("a space in the role name", format!("arn:aws:iam::{account}:role/Addon Role")),
        (
            "a character outside the name set",
            format!("arn:aws:iam::{account}:role/Addon*Role"),
        ),
        (
            "a role name one character past its maximum",
            format!("arn:aws:iam::{account}:role/{}", "r".repeat(65)),
        ),
        (
            "an empty path segment",
            format!("arn:aws:iam::{account}:role//AddonRole"),
        ),
        (
            "a space in the path",
            format!("arn:aws:iam::{account}:role/service role/AddonRole"),
        ),
    ];

    for (how, role) in &refused {
        for (operation, method, suffix) in [
            ("CreateAddon", "POST", "/addons".to_string()),
            (
                "UpdateAddon",
                "POST",
                format!("/addons/{}/update", encode_label(ADDON)),
            ),
        ] {
            for cluster in [GHOST, CLUSTER] {
                let case = format!("{operation} with {how} on cluster {cluster:?}");
                let body = serde_json::json!({
                    "addonName": ADDON,
                    "serviceAccountRoleArn": role,
                });
                let answer = call(
                    &router,
                    REGION,
                    method,
                    &cluster_path(cluster, &suffix),
                    &body.to_string(),
                )
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
                assert_eq!(
                    answer.body["message"],
                    serde_json::json!(ADDON_SERVICE_ACCOUNT_ROLE_ARN_REFUSAL),
                    "case {case}: expected the add-on role sentence, got {:?}",
                    answer.body["message"]
                );
                assert!(
                    answer.body.get("clusterName").is_none(),
                    "case {case}: the refusal names no resource, got {:?}",
                    answer.body["clusterName"]
                );
            }
        }
    }

    // The control: a role inside the grammar reaches its own stages — the missing
    // cluster on the ghost, and the operation itself on the cluster that exists. The
    // rows differ along the form the grammar DOES admit: a plain name, a name carrying
    // every punctuation character the name set allows, and a name under a path.
    let accepted = [
        ("a plain role name", format!("arn:aws:iam::{account}:role/AddonRole")),
        (
            "every punctuation character the name set allows",
            format!("arn:aws:iam::{account}:role/Addon_role+of=mine,v.1@eks-west"),
        ),
        (
            "a role under a path",
            format!("arn:aws:iam::{account}:role/service-role/eks.amazonaws.com/AddonRole"),
        ),
    ];
    // One add-on name installs at most once per cluster, so each accepted install
    // names a different add-on the catalog publishes.
    let installable = ["coredns", "kube-proxy", "metrics-server"];
    for (index, (how, role)) in accepted.iter().enumerate() {
        let body =
            serde_json::json!({ "addonName": installable[index], "serviceAccountRoleArn": role });
        let missing = call(
            &router,
            REGION,
            "POST",
            &cluster_path(GHOST, "/addons"),
            &body.to_string(),
        )
        .await;
        assert_eq!(
            missing.status, 404,
            "case CreateAddon with {how} on an unknown cluster: expected 404, got {missing:?}"
        );
        let served = call(
            &router,
            REGION,
            "POST",
            &cluster_path(CLUSTER, "/addons"),
            &body.to_string(),
        )
        .await;
        assert_eq!(
            served.status, 200,
            "case CreateAddon with {how} on an existing cluster: expected 200, got {served:?}"
        );
    }
}

/// The same question IAM is the one to answer: a role of this account that IAM does
/// NOT hold cannot be given to the add-on's service account either, and that is the
/// same refusal, given before the cluster is resolved.
///
/// The rows differ along which add-on operation carries the role and whether the
/// cluster the request is addressed to exists. The positive control is the same
/// request naming a role IAM DOES hold — created here through IAM's own `CreateRole`,
/// so the refusal is grounded in what IAM answered rather than in a value this suite
/// wrote. An emulator with no IAM mounted beside this service refuses nothing on this
/// question, which is what the suites above drive.
#[tokio::test]
async fn a_role_iam_does_not_hold_is_refused_before_the_cluster() {
    let router = emulator_with_identity();
    active_cluster(&router, CLUSTER).await;

    let account = engine::emulator_account_id();
    let held = format!("arn:aws:iam::{account}:role/AddonServiceAccountRole");
    let absent = format!("arn:aws:iam::{account}:role/NoSuchAddonRole");
    let trust = serde_json::json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": format!("arn:aws:iam::{account}:root")},
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    let (status, body) = query(
        &router,
        "iam",
        "2010-05-08",
        "CreateRole",
        &[
            ("RoleName", "AddonServiceAccountRole".to_string()),
            ("AssumeRolePolicyDocument", trust),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "setup CreateRole must succeed, got {status} {body}"
    );
    active_addon(&router, CLUSTER, ADDON).await;

    for (operation, method, suffix) in [
        ("CreateAddon", "POST", "/addons".to_string()),
        (
            "UpdateAddon",
            "POST",
            format!("/addons/{}/update", encode_label(ADDON)),
        ),
    ] {
        for cluster in [GHOST, CLUSTER] {
            let case = format!("{operation} with a role IAM does not hold on cluster {cluster:?}");
            let body = serde_json::json!({
                "addonName": ADDON,
                "serviceAccountRoleArn": absent,
            });
            let answer = call(
                &router,
                REGION,
                method,
                &cluster_path(cluster, &suffix),
                &body.to_string(),
            )
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
            assert_eq!(
                answer.body["message"],
                serde_json::json!(ADDON_SERVICE_ACCOUNT_ROLE_ARN_REFUSAL),
                "case {case}: expected the add-on role sentence, got {:?}",
                answer.body["message"]
            );
            assert!(
                answer.body.get("clusterName").is_none(),
                "case {case}: the refusal names no resource, got {:?}",
                answer.body["clusterName"]
            );
        }

        // The same request naming the role IAM HOLDS reaches its own stages: the
        // missing cluster on the ghost, and the operation itself on the cluster that
        // exists. The install names an add-on this cluster does not hold yet, since one
        // add-on name installs at most once per cluster; the update names the one that
        // is installed.
        let target = if suffix == "/addons" { "coredns" } else { ADDON };
        let body = serde_json::json!({ "addonName": target, "serviceAccountRoleArn": held });
        let missing = call(
            &router,
            REGION,
            method,
            &cluster_path(GHOST, &suffix),
            &body.to_string(),
        )
        .await;
        assert_eq!(
            missing.status, 404,
            "case {operation} with a role IAM holds on an unknown cluster: expected 404, \
             got {missing:?}"
        );
        let served = call(
            &router,
            REGION,
            method,
            &cluster_path(CLUSTER, &suffix),
            &body.to_string(),
        )
        .await;
        assert_eq!(
            served.status, 200,
            "case {operation} with a role IAM holds on an existing cluster: expected 200, \
             got {served:?}"
        );
        // One add-on takes one update at a time, so the update just started is waited
        // out before the next row starts another.
        tokio::time::sleep(ADDON_ACTIVATION_DELAY + Duration::from_millis(250)).await;
    }
}

// ---------------------------------------------------------------------------
// STAGE 2 before STAGE 3: `resolveConflicts` says what to do with a copy of the
// add-on already running ON the cluster, so it is a question about the add-on and is
// judged AFTER the cluster has been resolved: a request against a cluster that does
// not exist is that 404 whatever its `resolveConflicts` says, and the refusal names no
// add-on.
// ---------------------------------------------------------------------------

/// The table spans what the rule generalises over: which add-on operation was sent the
/// value, and HOW the value spells no declared option — unknown, a declared value with
/// padding, a comma-joined pair, and the empty string. The accepted side (a declared
/// value, in any case) is the `addon_request_validation` suite's.
#[tokio::test]
async fn the_cluster_is_resolved_before_resolve_conflicts_is_judged() {
    let router = emulator();
    active_cluster(&router, CLUSTER).await;
    active_addon(&router, CLUSTER, ADDON).await;

    for value in ["BANANA", " NONE", "NONE,OVERWRITE", ""] {
        for (operation, method, suffix) in [
            ("CreateAddon", "POST", "/addons".to_string()),
            (
                "UpdateAddon",
                "POST",
                format!("/addons/{}/update", encode_label(ADDON)),
            ),
        ] {
            let body = serde_json::json!({ "addonName": ADDON, "resolveConflicts": value });
            // On a cluster that does not exist: the missing cluster, named, and no
            // add-on member at all.
            let case = format!("{operation} with resolveConflicts {value:?} on an unknown cluster");
            let answer = call(
                &router,
                REGION,
                method,
                &cluster_path(GHOST, &suffix),
                &body.to_string(),
            )
            .await;
            assert_eq!(
                answer.status, 404,
                "case {case}: expected HTTP 404, got {answer:?}"
            );
            let code = error_code(&answer);
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case}: expected ResourceNotFoundException, got {code:?}"
            );
            assert!(
                answer.body.get("addonName").is_none(),
                "case {case}: the missing-cluster refusal names no add-on, got {:?}",
                answer.body["addonName"]
            );

            // On the cluster that DOES exist, the same value is refused as the value it
            // is: the order does not swallow the refusal, it only sequences it.
            let case = format!("{operation} with resolveConflicts {value:?} on an existing cluster");
            let refused = call(
                &router,
                REGION,
                method,
                &cluster_path(CLUSTER, &suffix),
                &body.to_string(),
            )
            .await;
            assert_eq!(
                refused.status, 400,
                "case {case}: expected HTTP 400, got {refused:?}"
            );
            let code = error_code(&refused);
            assert_eq!(
                code, "InvalidParameterException",
                "case {case}: expected InvalidParameterException, got {code:?}"
            );
            let expected = format!(
                "Resolve conflicts option {value} is not supported for operation {operation}"
            );
            assert_eq!(
                refused.body["message"],
                serde_json::json!(expected),
                "case {case}: expected message {expected:?}, got {:?}",
                refused.body["message"]
            );
        }
    }

    // A cluster that does not exist is reported for the cluster even when the add-on
    // the request names is not one the catalog publishes: both the add-on name and the
    // build it asks for are stage-3 questions.
    for (how, body) in [
        (
            "an add-on nothing publishes",
            serde_json::json!({ "addonName": "definitely-not-an-addon" }),
        ),
        (
            "a build the catalog does not offer",
            serde_json::json!({ "addonName": ADDON, "addonVersion": "v9.9.9-eksbuild.9" }),
        ),
    ] {
        let answer = call(
            &router,
            REGION,
            "POST",
            &cluster_path(GHOST, "/addons"),
            &body.to_string(),
        )
        .await;
        let case = format!("CreateAddon with {how} on an unknown cluster");
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404, got {answer:?}"
        );
        let expected = cluster_not_found_message(GHOST);
        assert_eq!(
            answer.body["message"],
            serde_json::json!(expected),
            "case {case}: expected message {expected:?}, got {:?}",
            answer.body["message"]
        );
    }
}
