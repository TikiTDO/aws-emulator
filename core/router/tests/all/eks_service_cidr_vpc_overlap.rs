// SPDX-License-Identifier: CC0-1.0

//! A cluster's Kubernetes service CIDR must not overlap the address space of the VPC
//! the cluster is created in — "Doesn't overlap with any CIDR block assigned to the
//! VPC that you selected"
//! (`KubernetesNetworkConfigRequest$serviceIpv4Cidr`).
//!
//! The rule needs EKS and EC2 on one engine, which is why this suite lives here
//! rather than in the EKS crate: the VPC a cluster is placed in is EC2's resource,
//! so its ranges are what EKS reads before it provisions anything, and an emulator
//! with no EC2 has nothing to read. Pod addresses come from the subnets' own ranges,
//! so a service range inside the VPC's would collide with them.
//!
//! The table spans what the rule generalises over: whether the service CIDR is the
//! VPC's own block, a sub-range of it, a super-range containing it, a block of an
//! ADDITIONAL range associated with the VPC, or a private block of another range
//! altogether — and the VPCs differ in their primary range, so no row is about one
//! recorded CIDR. Each refused row is then shown to leave no cluster, and each
//! served row to store the block it named.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built —
//! and the requests driven — on a sized thread.
//!
//! The request harness is the test crate's own: `eks_wire` for the drivers and the
//! answer, `query_wire` for the XML readers.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};

use crate::eks_wire::{ec2, eks, role_arn};
use crate::query_wire::{element, opt_element};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// One row: the service CIDR a create names, and the VPC range it must be reported
/// as overlapping — `None` when Amazon EKS serves the create.
struct Row {
    case: &'static str,
    cluster: &'static str,
    /// Which of the two VPCs the setup created the cluster is placed in.
    network: usize,
    service_cidr: &'static str,
    /// The VPC range the refusal names, or `None` for a create that is served.
    overlaps: Option<&'static str>,
}

const ROWS: &[Row] = &[
    Row {
        case: "the VPC's own primary range",
        cluster: "svc-cidr-vpc-range",
        network: 0,
        service_cidr: "10.21.0.0/16",
        overlaps: Some("10.21.0.0/16"),
    },
    Row {
        case: "a sub-range of the VPC's primary range",
        cluster: "svc-cidr-inside",
        network: 0,
        service_cidr: "10.21.3.0/24",
        overlaps: Some("10.21.0.0/16"),
    },
    Row {
        case: "a super-range containing the VPC's primary range",
        cluster: "svc-cidr-around",
        network: 0,
        service_cidr: "10.16.0.0/12",
        overlaps: Some("10.21.0.0/16"),
    },
    Row {
        case: "an additional range associated with the VPC",
        cluster: "svc-cidr-secondary",
        network: 0,
        service_cidr: "10.22.0.0/16",
        overlaps: Some("10.22.0.0/16"),
    },
    Row {
        case: "the second VPC's own range, for a cluster placed in it",
        cluster: "svc-cidr-second-vpc",
        network: 1,
        service_cidr: "192.168.0.0/16",
        overlaps: Some("192.168.0.0/16"),
    },
    Row {
        case: "a private block of another range than the first VPC's",
        cluster: "svc-cidr-elsewhere",
        network: 0,
        service_cidr: "172.20.0.0/16",
        overlaps: None,
    },
    Row {
        case: "the first VPC's range, for a cluster placed in the SECOND VPC",
        cluster: "svc-cidr-other-vpc-range",
        network: 1,
        service_cidr: "10.21.0.0/16",
        overlaps: None,
    },
];

#[test]
fn eks_refuses_a_service_cidr_that_overlaps_the_vpc_it_creates_the_cluster_in() {
    std::thread::Builder::new()
        .name("eks-service-cidr-vpc-overlap".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // ---------------------------------------------------------------
            // Setup: two VPCs with subnets of their own, through EC2 itself —
            // the first one carrying a SECOND associated range, so a row can
            // overlap a block that is not the primary one.
            // ---------------------------------------------------------------
            let mut networks: Vec<Vec<String>> = Vec::new();
            for (index, (vpc_cidr, subnet_cidrs)) in [
                ("10.21.0.0/16", ["10.21.1.0/24", "10.21.2.0/24"]),
                ("192.168.0.0/16", ["192.168.1.0/24", "192.168.2.0/24"]),
            ]
            .into_iter()
            .enumerate()
            {
                let created = ec2(&router, &runtime, "CreateVpc", &[("CidrBlock", vpc_cidr)]);
                assert_eq!(
                    created.status, 200,
                    "setup: CreateVpc {vpc_cidr} must succeed, got {created:?}"
                );
                let vpc_id = element(&created.text, "vpcId");
                if index == 0 {
                    let associated = ec2(
                        &router,
                        &runtime,
                        "AssociateVpcCidrBlock",
                        &[("VpcId", vpc_id.as_str()), ("CidrBlock", "10.22.0.0/16")],
                    );
                    assert_eq!(
                        associated.status, 200,
                        "setup: AssociateVpcCidrBlock must succeed, got {associated:?}"
                    );
                }
                let mut subnets = Vec::new();
                for cidr_block in subnet_cidrs {
                    let answer = ec2(
                        &router,
                        &runtime,
                        "CreateSubnet",
                        &[("VpcId", vpc_id.as_str()), ("CidrBlock", cidr_block)],
                    );
                    assert_eq!(
                        answer.status, 200,
                        "setup: CreateSubnet {cidr_block} must succeed, got {answer:?}"
                    );
                    subnets.push(element(&answer.text, "subnetId"));
                }
                networks.push(subnets);
            }

            for row in ROWS {
                let subnets = networks
                    .get(row.network)
                    .expect("every row names a network the setup created");
                let body = serde_json::json!({
                    "name": row.cluster,
                    "roleArn": role_arn("eksClusterRole"),
                    "resourcesVpcConfig": { "subnetIds": subnets },
                    "kubernetesNetworkConfig": { "serviceIpv4Cidr": row.service_cidr },
                });
                let answer = eks(&router, &runtime, "POST", "/clusters", Some(body));
                let read = eks(
                    &router,
                    &runtime,
                    "GET",
                    &format!("/clusters/{}", row.cluster),
                    None,
                );
                match row.overlaps {
                    Some(vpc_range) => {
                        assert_eq!(
                            (answer.status, answer.error_type.as_deref()),
                            (400, Some("InvalidParameterException")),
                            "case {:?}: an overlapping service CIDR must be refused 400 \
                             InvalidParameterException, got {answer:?}",
                            row.case
                        );
                        let expected = format!(
                            "Invalid Cluster Service Ipv4 CIDR {} for the cluster {}, overlaps \
                             with VPC range {vpc_range}",
                            row.service_cidr, row.cluster
                        );
                        assert_eq!(
                            answer.message(),
                            expected,
                            "case {:?}: expected message {expected:?}, got {:?}",
                            row.case,
                            answer.message()
                        );
                        // And the refused create left no cluster behind.
                        assert_eq!(
                            (read.status, read.error_type.as_deref()),
                            (404, Some("ResourceNotFoundException")),
                            "case {:?}: a refused create must leave no cluster, got {read:?}",
                            row.case
                        );
                    }
                    None => {
                        assert_eq!(
                            answer.status, 200,
                            "case {:?}: a service CIDR that overlaps nothing of the VPC must be \
                             served, got {answer:?}",
                            row.case
                        );
                        assert_eq!(
                            read.status, 200,
                            "case {:?}: the cluster must read back, got {read:?}",
                            row.case
                        );
                        assert_eq!(
                            read.body["cluster"]["kubernetesNetworkConfig"]["serviceIpv4Cidr"],
                            serde_json::json!(row.service_cidr),
                            "case {:?}: the cluster must report the service CIDR it was created \
                             with, got {}",
                            row.case,
                            read.text
                        );
                        // The cluster really sits in the VPC whose ranges were read.
                        assert!(
                            read.body["cluster"]["resourcesVpcConfig"]["vpcId"].is_string(),
                            "case {:?}: the cluster must report the VPC its subnets sit in, \
                             got {}",
                            row.case,
                            read.text
                        );
                    }
                }
            }

            // A VPC this emulator has no record of grounds no refusal, which is the
            // silence every other cross-service reading of this service treats the
            // same way: a create naming subnets EC2 does not hold is refused for the
            // SUBNETS, not for the service CIDR.
            let unknown = eks(
                &router,
                &runtime,
                "POST",
                "/clusters",
                Some(serde_json::json!({
                    "name": "svc-cidr-unknown-subnets",
                    "roleArn": role_arn("eksClusterRole"),
                    "resourcesVpcConfig": { "subnetIds": ["subnet-nope1", "subnet-nope2"] },
                    "kubernetesNetworkConfig": { "serviceIpv4Cidr": "10.21.0.0/16" },
                })),
            );
            assert_eq!(
                (unknown.status, unknown.error_type.as_deref()),
                (400, Some("InvalidParameterException")),
                "case unresolved subnets: expected 400 InvalidParameterException, got {unknown:?}"
            );
            assert!(
                unknown.message().contains("subnet-nope"),
                "case unresolved subnets: the refusal must be about the subnets, got {:?}",
                unknown.message()
            );
            assert!(
                opt_element(&unknown.text, "vpcId").is_none(),
                "case unresolved subnets: nothing of a VPC is reported, got {}",
                unknown.text
            );
        })
        .expect("spawn the eks service-cidr overlap thread")
        .join()
        .expect("the eks service-cidr overlap thread");
}
