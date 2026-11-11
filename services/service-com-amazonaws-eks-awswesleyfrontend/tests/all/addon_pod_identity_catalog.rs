// SPDX-License-Identifier: CC0-1.0

//! The pod-identity configuration the add-on catalog publishes, read over the wire
//! through `DescribeAddonConfiguration` on the same `MultiServiceRouter` the
//! production binary mounts.
//!
//! The RULE under test: a published add-on's `podIdentityConfiguration` names EVERY
//! Kubernetes service account that add-on's own workloads call AWS as — one entry per
//! service account, so a driver whose controller and node components both call AWS
//! carries both — each with the AWS-managed policies Amazon EKS recommends for that
//! service account, and an add-on whose workloads call AWS as none reports an empty
//! list. The list is a property of the ADD-ON, not of a build, so every published
//! build of one add-on reports the same list.
//!
//! Two suites assert it:
//!
//! * a TABLE of the recommendations the catalog publishes, whose rows differ along
//!   what the rule generalises over — how many service accounts an add-on runs as
//!   (none, one, two), how many policies are recommended for one of them (one, two),
//!   and the path depth of the policy ARNs (the account root and `/service-role/`) —
//!   each row read back at both the newest and the oldest build the add-on publishes;
//! * an invariant over the WHOLE catalog, every add-on the emulator publishes being a
//!   row: the entries of one add-on name distinct service accounts, every recommended
//!   policy is an ARN in AWS's own managed-policy namespace, and two builds of one
//!   add-on cannot disagree about the list.
//!
//! The harness is the crate's own (`wire.rs`), and the catalog readers are the ones
//! `addon_versions.rs` owns.

use std::collections::BTreeSet;

use smithy_server_runtime::MultiServiceRouter;

use crate::addon_catalog_publication::{resolved, schema_root};
use crate::addon_versions::{
    addons, describe_addon_configuration, string_member, versions_of, whole_catalog,
};
use crate::wire::emulator;

/// The prefix of an ARN in AWS's own managed-policy namespace.
const AWS_MANAGED_POLICY_PREFIX: &str = "arn:aws:iam::aws:policy/";

/// What one published add-on's `podIdentityConfiguration` must report: the service
/// accounts it runs as, in the order the catalog publishes them, each with the
/// managed policies recommended for it.
#[derive(Debug)]
struct Expected {
    addon: &'static str,
    pod_identity: &'static [(&'static str, &'static [&'static str])],
}

/// The recommendations the catalog publishes.
///
/// SOURCE: the "Amazon EKS add-ons" section of the Amazon EKS User Guide, which
/// lists for each add-on supporting EKS Pod Identity the Kubernetes service
/// accounts its workloads run as and the AWS-managed policy recommended for each —
/// the external authority this expectation is taken from, not the catalog under
/// test. Rows are written in add-on name order; the add-ons with no entry at all
/// are the tail of the table.
const EXPECTED: &[Expected] = &[
    Expected {
        // One service account, TWO recommended policies: the agent publishes both
        // metrics and traces.
        addon: "amazon-cloudwatch-observability",
        pod_identity: &[(
            "cloudwatch-agent",
            &[
                "arn:aws:iam::aws:policy/CloudWatchAgentServerPolicy",
                "arn:aws:iam::aws:policy/AWSXrayWriteOnlyAccess",
            ],
        )],
    },
    Expected {
        // An operator that calls AWS for the work it runs, so it carries the
        // managed policy Amazon Web Services publishes for it.
        addon: "amazon-sagemaker-hyperpod-training-operator",
        pod_identity: &[(
            "hp-training-controller-manager",
            &["arn:aws:iam::aws:policy/AmazonSageMakerHyperPodTrainingOperatorAccess"],
        )],
    },
    Expected {
        // The EBS driver calls AWS from its controller only, so one entry.
        addon: "aws-ebs-csi-driver",
        pod_identity: &[(
            "ebs-csi-controller-sa",
            &["arn:aws:iam::aws:policy/service-role/AmazonEBSCSIDriverPolicy"],
        )],
    },
    Expected {
        // The EFS driver calls Amazon EFS from BOTH components, so both of its
        // service accounts are named.
        addon: "aws-efs-csi-driver",
        pod_identity: &[
            (
                "efs-csi-controller-sa",
                &["arn:aws:iam::aws:policy/service-role/AmazonEFSCSIDriverPolicy"],
            ),
            (
                "efs-csi-node-sa",
                &["arn:aws:iam::aws:policy/service-role/AmazonEFSCSIDriverPolicy"],
            ),
        ],
    },
    Expected {
        addon: "aws-network-flow-monitoring-agent",
        pod_identity: &[(
            "aws-network-flow-monitor-agent",
            &["arn:aws:iam::aws:policy/CloudWatchNetworkFlowMonitorAgentPublishPolicy"],
        )],
    },
    Expected {
        addon: "external-dns",
        pod_identity: &[(
            "external-dns",
            &["arn:aws:iam::aws:policy/AmazonRoute53FullAccess"],
        )],
    },
    Expected {
        addon: "vpc-cni",
        pod_identity: &[("aws-node", &["arn:aws:iam::aws:policy/AmazonEKS_CNI_Policy"])],
    },
    // Add-ons whose workloads call no AWS API of their own: the list is empty, not
    // missing, and the published schema declares no service account default.
    Expected {
        addon: "coredns",
        pod_identity: &[],
    },
    Expected {
        addon: "eks-pod-identity-agent",
        pod_identity: &[],
    },
    Expected {
        addon: "kube-proxy",
        pod_identity: &[],
    },
];

/// One add-on's reported `podIdentityConfiguration`, as `(serviceAccount, policies)`
/// pairs in the order reported.
fn reported_pod_identity(
    body: &serde_json::Value,
    case: &str,
) -> Vec<(String, Vec<String>)> {
    body["podIdentityConfiguration"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a podIdentityConfiguration list, got {body:?}"))
        .iter()
        .map(|entry| {
            let policies = entry["recommendedManagedPolicies"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case {case}: expected a recommendedManagedPolicies list, got {entry:?}")
                })
                .iter()
                .map(|policy| {
                    policy
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("case {case}: expected a policy ARN string, got {policy:?}")
                        })
                        .to_string()
                })
                .collect();
            (string_member(entry, "serviceAccount"), policies)
        })
        .collect()
}

/// The newest and the oldest build the catalog publishes for `addon_name`, which is
/// how each row is read at two builds of the same add-on.
async fn newest_and_oldest_build(router: &MultiServiceRouter, addon_name: &str) -> Vec<String> {
    let catalog = addons(&whole_catalog(router).await);
    let addon = catalog
        .iter()
        .find(|addon| string_member(addon, "addonName") == addon_name)
        .unwrap_or_else(|| panic!("the catalog must publish {addon_name}"));
    let builds = versions_of(addon);
    let newest = string_member(builds.first().expect("a published build"), "addonVersion");
    let oldest = string_member(builds.last().expect("a published build"), "addonVersion");
    if newest == oldest {
        vec![newest]
    } else {
        vec![newest, oldest]
    }
}

/// The `serviceAccount.name` default the published configuration schema declares, or
/// `None` when the schema declares no service account at all.
///
/// The document is a `$ref` into its own `definitions`, so both the root type and
/// the service-account type are read through the references that name them.
fn schema_service_account_default(body: &serde_json::Value, case: &str) -> Option<String> {
    let schema: serde_json::Value =
        serde_json::from_str(&string_member(body, "configurationSchema"))
            .unwrap_or_else(|error| panic!("case {case}: configurationSchema must be JSON: {error}"));
    let root = schema_root(&schema);
    let declared = &root["properties"]["serviceAccount"];
    if declared.is_null() {
        return None;
    }
    resolved(&schema, declared)["properties"]["name"]["default"]
        .as_str()
        .map(str::to_string)
}

#[tokio::test]
async fn every_service_account_an_addon_runs_as_is_published_with_its_policies() {
    let router = emulator();

    for expected in EXPECTED {
        for build in newest_and_oldest_build(&router, expected.addon).await {
            let case = format!("{}/{build}", expected.addon);
            let answer = describe_addon_configuration(&router, expected.addon, &build).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected HTTP 200, got {answer:?}"
            );

            let reported = reported_pod_identity(&answer.body, &case);
            let reported_accounts: Vec<&str> = reported
                .iter()
                .map(|(account, _)| account.as_str())
                .collect();
            let expected_accounts: Vec<&str> = expected
                .pod_identity
                .iter()
                .map(|(account, _)| *account)
                .collect();
            assert_eq!(
                reported_accounts, expected_accounts,
                "case {case}: expected the service accounts the add-on runs as, got \
                 {reported_accounts:?}"
            );
            for ((account, policies), (_, expected_policies)) in
                reported.iter().zip(expected.pod_identity)
            {
                let policies: Vec<&str> = policies.iter().map(String::as_str).collect();
                assert_eq!(
                    policies.as_slice(),
                    *expected_policies,
                    "case {case}/{account}: expected the policies recommended for it, got \
                     {policies:?}"
                );
            }

            // The schema's service account default is the add-on's FIRST published
            // entry, and an add-on that runs as none declares no default: a second
            // entry adds a recommendation, it does not change the install default.
            let default = schema_service_account_default(&answer.body, &case);
            assert_eq!(
                default.as_deref(),
                expected_accounts.first().copied(),
                "case {case}: expected the first published service account as the schema \
                 default, got {default:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_pod_identity_configuration_is_a_property_of_the_addon() {
    let router = emulator();
    let catalog = addons(&whole_catalog(&router).await);
    assert!(
        !catalog.is_empty(),
        "setup: the catalog must publish add-ons, got {catalog:?}"
    );

    for addon in &catalog {
        let addon_name = string_member(addon, "addonName");
        let mut across_builds: Option<Vec<(String, Vec<String>)>> = None;
        for build in versions_of(addon) {
            let addon_version = string_member(&build, "addonVersion");
            let case = format!("{addon_name}/{addon_version}");
            let answer = describe_addon_configuration(&router, &addon_name, &addon_version).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected HTTP 200, got {answer:?}"
            );
            let reported = reported_pod_identity(&answer.body, &case);

            let accounts: BTreeSet<&str> = reported
                .iter()
                .map(|(account, _)| account.as_str())
                .collect();
            assert_eq!(
                accounts.len(),
                reported.len(),
                "case {case}: one service account is named at most once, got {reported:?}"
            );
            for (account, policies) in &reported {
                assert!(
                    !policies.is_empty(),
                    "case {case}/{account}: a published service account carries the policies \
                     recommended for it, got {policies:?}"
                );
                for policy in policies {
                    assert!(
                        policy.starts_with(AWS_MANAGED_POLICY_PREFIX)
                            && policy.len() > AWS_MANAGED_POLICY_PREFIX.len(),
                        "case {case}/{account}: a recommended policy is an AWS-managed policy \
                         ARN, got {policy:?}"
                    );
                }
            }

            match &across_builds {
                None => across_builds = Some(reported),
                Some(first) => assert_eq!(
                    &reported, first,
                    "case {case}: every build of one add-on reports the same pod identity \
                     configuration, got {reported:?} against {first:?}"
                ),
            }
        }
    }
}
