// SPDX-License-Identifier: CC0-1.0

//! The pass-role authorization every operation of this service applies to a role ARN
//! the caller hands to Amazon EKS.
//!
//! RULE: a role a request asks Amazon EKS to assume is passed to the service with the
//! caller's own `iam:PassRole`, so it must name a role of the caller's OWN account and
//! partition. Anything else is refused `AccessDeniedException` (403) — with the fixed
//! cross-account sentence when the value names no role of this account at all (which is
//! what the recorded service answers, even for a value that is not an ARN), and with a
//! message naming the value when it is an in-account IAM ARN that names something other
//! than a role, or a role of another partition. Either way NOTHING is created: the check
//! runs before the request's body is reported on and before anything is written.
//!
//! The rows span what the rule generalises over: which account and partition the ARN
//! names, what within IAM it names, the role's path, the forms that are not IAM role
//! ARNs — and, across the suites, every member of this service that carries such a role
//! (`CreateCluster$roleArn`, `CreateCluster$computeConfig.nodeRoleArn`,
//! `CreateNodegroup$nodeRole`, `CreateCapability$roleArn`,
//! `CreatePodIdentityAssociation$roleArn`,
//! `RegisterCluster$connectorConfig.roleArn` — the last of which the recording shows
//! reporting the SAME question as its own 400 `InvalidParameterException`, so its suite
//! asserts that shape instead).
//!
//! The wire harness (`crate::wire`) and the per-operation helpers the other suites own
//! are reused as they are.

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    CONNECTOR_ROLE_ARN_REFUSAL, CROSS_ACCOUNT_PASS_ROLE_REFUSAL,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{describe, REGION};
use crate::wire::{call, emulator, encode_label, Answer};

/// What the rule answers for one role ARN.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// The role may be passed: the request is served on its own merits.
    Passed,
    /// The value names no role of this account at all: 403 with the fixed sentence.
    CrossAccount,
    /// An IAM ARN of this account that still cannot be passed: 403 with a message
    /// naming the value.
    NamedRefusal,
}

impl Verdict {
    fn status(self) -> u16 {
        match self {
            Self::Passed => 200,
            Self::CrossAccount | Self::NamedRefusal => 403,
        }
    }

    fn error_type(self) -> Option<&'static str> {
        match self {
            Self::Passed => None,
            Self::CrossAccount | Self::NamedRefusal => Some("AccessDeniedException"),
        }
    }
}

/// The account every ARN this emulator mints carries, so a row naming it names a role
/// of the caller's own account.
fn own_account() -> &'static str {
    engine::emulator_account_id()
}

/// One row: a role ARN and what the rule answers for it.
///
/// The rows differ along which account and partition the ARN names, what it names
/// within IAM, the role's path, and the forms that are not IAM role ARNs at all.
fn role_rows() -> Vec<(&'static str, String, Verdict)> {
    let own = own_account();
    vec![
        (
            "a role of this account",
            format!("arn:aws:iam::{own}:role/eksPassableRole"),
            Verdict::Passed,
        ),
        (
            "a role of this account under a path",
            format!("arn:aws:iam::{own}:role/service-role/eksPassableRole"),
            Verdict::Passed,
        ),
        (
            "a role of another account",
            "arn:aws:iam::111122223333:role/anything".to_string(),
            Verdict::CrossAccount,
        ),
        (
            "a role of another account spelled like one of this account",
            "arn:aws:iam::210987654321:role/eksPassableRole".to_string(),
            Verdict::CrossAccount,
        ),
        (
            "a role of another partition",
            format!("arn:aws-cn:iam::{own}:role/eksPassableRole"),
            Verdict::NamedRefusal,
        ),
        (
            "an IAM user, which no service assumes",
            format!("arn:aws:iam::{own}:user/operator"),
            Verdict::NamedRefusal,
        ),
        (
            "an IAM ARN naming no role at all",
            format!("arn:aws:iam::{own}:role/"),
            Verdict::NamedRefusal,
        ),
        (
            "another vendor's ARN",
            format!("arn:aws:eks:{REGION}:{own}:cluster/other"),
            Verdict::CrossAccount,
        ),
        (
            "an account that is not a 12-digit account id",
            "arn:aws:iam::12345:role/short-account".to_string(),
            Verdict::CrossAccount,
        ),
        (
            "a bare role name",
            "eksPassableRole".to_string(),
            Verdict::CrossAccount,
        ),
        (
            "a value that is not an ARN at all",
            "not-an-arn".to_string(),
            Verdict::CrossAccount,
        ),
        ("the empty string", String::new(), Verdict::CrossAccount),
    ]
}

/// Assert one row's answer, and that the fixed cross-account sentence is reserved for
/// the values that name no role of this account.
fn assert_row(case: &str, role_arn: &str, verdict: Verdict, answer: &Answer) {
    assert_eq!(
        answer.status,
        verdict.status(),
        "case {case:?} ({role_arn}): expected HTTP {}, got {answer:?}",
        verdict.status()
    );
    assert_eq!(
        answer.error_type.as_deref(),
        verdict.error_type(),
        "case {case:?} ({role_arn}): expected error {:?}, got {answer:?}",
        verdict.error_type()
    );
    let actual = answer.message();
    match verdict {
        Verdict::Passed => {}
        Verdict::CrossAccount => assert_eq!(
            actual, CROSS_ACCOUNT_PASS_ROLE_REFUSAL,
            "case {case:?} ({role_arn}): a value naming no role of this account earns the \
             fixed cross-account refusal, got {actual:?}"
        ),
        Verdict::NamedRefusal => {
            assert_ne!(
                actual, CROSS_ACCOUNT_PASS_ROLE_REFUSAL,
                "case {case:?} ({role_arn}): this ARN is in THIS account, so the refusal \
                 must say what else was wrong, got {actual:?}"
            );
            assert!(
                actual.contains(role_arn),
                "case {case:?}: the refusal must name the role at fault, got {actual:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// `CreateCluster$roleArn`: the cluster's service role.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_cluster_passes_only_a_cluster_role_of_this_account() {
    for (index, (case, role_arn, verdict)) in role_rows().into_iter().enumerate() {
        let router = emulator();
        let name = format!("role-case-{index}");
        let body = serde_json::json!({
            "name": name,
            "roleArn": role_arn,
            "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"] },
        });
        let answer = call(&router, REGION, "POST", "/clusters", &body.to_string()).await;
        assert_row(case, &role_arn, verdict, &answer);

        // A refused create wrote nothing; an accepted one is there to read.
        let expected_read = if verdict == Verdict::Passed { 200 } else { 404 };
        let described = describe(&router, &name).await;
        assert_eq!(
            described.status, expected_read,
            "case {case:?}: expected DescribeCluster to answer {expected_read}, got \
             {described:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// `CreateCluster$computeConfig.nodeRoleArn`: the EKS Auto Mode node role, passed on
// the same request. The same rule, on a member the recorded case never carried.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_cluster_passes_only_an_auto_mode_node_role_of_this_account() {
    let passable = format!("arn:aws:iam::{}:role/eksClusterRole", own_account());
    for (index, (case, role_arn, verdict)) in role_rows().into_iter().enumerate() {
        let router = emulator();
        let name = format!("compute-case-{index}");
        let body = serde_json::json!({
            "name": name,
            "roleArn": passable,
            "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"] },
            // EKS Auto Mode is one mode made of three capabilities, which Amazon EKS
            // requires to be enabled together, so the request that carries the node
            // role enables all three: the role is what this suite is about, and the
            // rest of the body has to be a configuration a cluster can have.
            "computeConfig": {
                "enabled": true,
                "nodePools": ["general-purpose"],
                "nodeRoleArn": role_arn,
            },
            "storageConfig": { "blockStorage": { "enabled": true } },
            "kubernetesNetworkConfig": { "elasticLoadBalancing": { "enabled": true } },
        });
        let answer = call(&router, REGION, "POST", "/clusters", &body.to_string()).await;
        assert_row(case, &role_arn, verdict, &answer);
        let expected_read = if verdict == Verdict::Passed { 200 } else { 404 };
        let described = describe(&router, &name).await;
        assert_eq!(
            described.status, expected_read,
            "case {case:?}: expected DescribeCluster to answer {expected_read}, got \
             {described:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// `CreateNodegroup$nodeRole`: the role Amazon EKS passes to the nodes it launches.
// ---------------------------------------------------------------------------

/// A cluster for a node group to hang off, created with a role of this account.
async fn cluster_for_nodegroups(router: &MultiServiceRouter, name: &str) {
    let body = serde_json::json!({
        "name": name,
        "roleArn": format!("arn:aws:iam::{}:role/eksClusterRole", own_account()),
        "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"] },
    });
    let answer = call(router, REGION, "POST", "/clusters", &body.to_string()).await;
    assert_eq!(
        answer.status, 200,
        "setup: the cluster {name:?} a node group needs must be created, got {answer:?}"
    );
}

#[tokio::test]
async fn create_nodegroup_passes_only_a_node_role_of_this_account() {
    let cluster = "nodegroup-role-cluster";
    for (index, (case, role_arn, verdict)) in role_rows().into_iter().enumerate() {
        let router = emulator();
        cluster_for_nodegroups(&router, cluster).await;
        let name = format!("ng-role-case-{index}");
        let body = serde_json::json!({
            "nodegroupName": name,
            "nodeRole": role_arn,
            "subnets": ["subnet-aaaa1111", "subnet-bbbb2222"],
        });
        let answer = call(
            &router,
            REGION,
            "POST",
            &format!("/clusters/{}/node-groups", encode_label(cluster)),
            &body.to_string(),
        )
        .await;
        assert_row(case, &role_arn, verdict, &answer);

        let described = call(
            &router,
            REGION,
            "GET",
            &format!(
                "/clusters/{}/node-groups/{}",
                encode_label(cluster),
                encode_label(&name)
            ),
            "",
        )
        .await;
        let expected_read = if verdict == Verdict::Passed { 200 } else { 404 };
        assert_eq!(
            described.status, expected_read,
            "case {case:?}: expected DescribeNodegroup to answer {expected_read}, got \
             {described:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// `CreateCapability$roleArn`: the role the capability has Amazon EKS assume.
//
// The authorization precedes every report about the cluster the request named, so an
// unpassable role is refused even when the cluster does not exist — which is what the
// second half of this suite asserts.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_capability_passes_only_a_capability_role_of_this_account() {
    // ONE router, and one cluster per row — a cluster holds at most one capability of a
    // type, so each row needs its own — with the clock-owed `CREATING` -> `ACTIVE` move
    // waited out ONCE for all of them.
    let router = emulator();
    let rows = role_rows();
    let clusters: Vec<String> = (0..rows.len())
        .map(|index| format!("capability-role-cluster-{index}"))
        .collect();
    for cluster in &clusters {
        cluster_for_nodegroups(&router, cluster).await;
    }
    tokio::time::sleep(
        service_com_amazonaws_eks_awswesleyfrontend::state::CLUSTER_ACTIVATION_DELAY,
    )
    .await;
    // A capability is only created on an ACTIVE cluster, and reading the cluster back is
    // what materialises the move the clock now owes it.
    for cluster in &clusters {
        let described = describe(&router, cluster).await;
        assert_eq!(
            described.cluster()["status"],
            serde_json::json!("ACTIVE"),
            "setup: {cluster} must be ACTIVE before a capability is created, got \
             {described:?}"
        );
    }

    for (index, (case, role_arn, verdict)) in rows.into_iter().enumerate() {
        let cluster = clusters[index].as_str();
        let body = serde_json::json!({
            "capabilityName": format!("cap-role-case-{index}"),
            "type": "ACK",
            "roleArn": role_arn,
            "deletePropagationPolicy": "RETAIN",
        });
        let answer = call(
            &router,
            REGION,
            "POST",
            &format!("/clusters/{}/capabilities", encode_label(cluster)),
            &body.to_string(),
        )
        .await;
        assert_row(case, &role_arn, verdict, &answer);
    }

    // The same rule, with no cluster at all: authorization is not reached through the
    // parent resource, so an unpassable role is still a 403 and never a 404.
    let router = emulator();
    let answer = call(
        &router,
        REGION,
        "POST",
        "/clusters/no-such-cluster/capabilities",
        &serde_json::json!({
            "capabilityName": "orphan",
            "type": "ACK",
            "roleArn": "arn:aws:iam::111122223333:role/anything",
            "deletePropagationPolicy": "RETAIN",
        })
        .to_string(),
    )
    .await;
    assert_row(
        "a role of another account on a cluster that does not exist",
        "arn:aws:iam::111122223333:role/anything",
        Verdict::CrossAccount,
        &answer,
    );
}

// ---------------------------------------------------------------------------
// `RegisterCluster$connectorConfig.roleArn`: the connector role.
//
// The same QUESTION — does the value name a role of this account? — reported the way
// the recording for THIS member shows it: 400 `InvalidParameterException` with the one
// sentence that rule carries, rather than the pass-role 403 above. A registration does
// not provision a control plane to pass a role to; the connector role is a parameter of
// the registration itself.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn register_cluster_refuses_every_connector_role_that_is_not_of_this_account() {
    for (index, (case, role_arn, verdict)) in role_rows().into_iter().enumerate() {
        let router = emulator();
        let name = format!("connector-role-case-{index}");
        let body = serde_json::json!({
            "name": name,
            "connectorConfig": { "roleArn": role_arn, "provider": "OTHER" },
        });
        let answer = call(
            &router,
            REGION,
            "POST",
            "/cluster-registrations",
            &body.to_string(),
        )
        .await;
        if verdict == Verdict::Passed {
            assert_eq!(
                answer.status, 200,
                "case {case:?} ({role_arn}): a role of this account registers, got {answer:?}"
            );
        } else {
            assert_eq!(
                (answer.status, answer.error_type.as_deref()),
                (400, Some("InvalidParameterException")),
                "case {case:?} ({role_arn}): expected 400 InvalidParameterException, got \
                 {answer:?}"
            );
            assert_eq!(
                answer.message(),
                CONNECTOR_ROLE_ARN_REFUSAL,
                "case {case:?} ({role_arn}): expected the connector-role refusal, got \
                 {answer:?}"
            );
        }
        let expected_read = if verdict == Verdict::Passed { 200 } else { 404 };
        let described = describe(&router, &name).await;
        assert_eq!(
            described.status, expected_read,
            "case {case:?}: expected DescribeCluster to answer {expected_read}, got \
             {described:?}"
        );
    }
}
