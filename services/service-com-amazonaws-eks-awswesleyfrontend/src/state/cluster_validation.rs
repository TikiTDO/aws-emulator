// SPDX-License-Identifier: CC0-1.0

//! The configurations Amazon EKS refuses a CLUSTER for, in one place.
//!
//! Every function here answers the SENTENCE a request earns, or `None` when the
//! request is one Amazon EKS serves. They are pure readings of the request (plus,
//! for the service CIDR, the VPC its subnets resolved to, and for the support type,
//! the published version calendar), so the operations that judge the same member —
//! `CreateCluster` for what a cluster is created with, `UpdateClusterConfig` for
//! what it is changed to — ask the same question through one implementation rather
//! than two.
//!
//! What is NOT here: the single-member parameter rules and their sentences, which
//! live in [`super::cluster_parameters`] (the cluster name's grammar, the two
//! endpoint-access switches, the `publicAccessCidrs` entries, the form of a subnet
//! or security group id, and the versions this service offers). This module holds
//! the rules that read SEVERAL members together, or a member against the network or
//! the calendar.
//!
//! The constraints come from the members' own documentation
//! (`smithy_query query="shape" shape="KubernetesNetworkConfigRequest,
//! ComputeConfigRequest,StorageConfigRequest,ElasticLoadBalancing,UpgradePolicyRequest"`)
//! and from the support lifecycle the version calendar publishes
//! ([`ClusterVersionRecord`]).

use super::*;

use ::std::str::FromStr;

/// The `SupportType` member that asks for no extended support: "Use `STANDARD` to
/// disable extended support" (`UpgradePolicyRequest`). It is one of the two members
/// [`crate::ops::types::SUPPORT_TYPE_VALUES`] carries, named here because a
/// cross-member rule is decided on it; the other is [`DEFAULT_SUPPORT_TYPE`], the
/// one a cluster gets when the request names none.
pub const SUPPORT_TYPE_STANDARD: &str = "STANDARD";

/// The private address blocks a cluster's Kubernetes service CIDR has to sit
/// inside: "Within one of the following private IP address blocks: `10.0.0.0/8`,
/// `172.16.0.0/12`, or `192.168.0.0/16`"
/// (`KubernetesNetworkConfigRequest$serviceIpv4Cidr`) — the RFC 1918 ranges.
pub const SERVICE_IPV4_PRIVATE_BLOCKS: &[&str] = &["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16"];

/// The widest netmask a service CIDR may carry: "Between `/24` and `/12`"
/// (`KubernetesNetworkConfigRequest$serviceIpv4Cidr`).
pub const SERVICE_IPV4_MIN_PREFIX: u8 = 12;

/// The narrowest netmask a service CIDR may carry — see
/// [`SERVICE_IPV4_MIN_PREFIX`].
pub const SERVICE_IPV4_MAX_PREFIX: u8 = 24;

/// The refusal deleting a cluster that still has managed node groups earns: "If
/// you have managed node groups or Fargate profiles attached to the cluster, you
/// must delete them first" (`DeleteCluster`). The whole sentence is fixed — it
/// says WHAT is attached, not which ones.
pub const CLUSTER_HAS_NODEGROUPS_ATTACHED: &str = "Cluster has nodegroups attached";

/// The refusal deleting a cluster that still has Fargate profiles earns — see
/// [`CLUSTER_HAS_NODEGROUPS_ATTACHED`]. A profile that exists is one Amazon EKS is
/// running or creating, which is the state this sentence names.
pub const CLUSTER_HAS_FARGATE_PROFILES_ATTACHED: &str =
    "Cluster has Fargate Profiles in ACTIVE/CREATING state";

/// The refusal a `DeleteCluster` earns while the deletion of that cluster is still
/// in flight: the cluster is not absent, it is busy being deleted, which is what
/// `ResourceInUseException` ("The specified resource is in use") reports. The
/// cluster's name is the variable part.
pub fn cluster_being_deleted(cluster_name: &str) -> String {
    format!("Cluster is being deleted: {cluster_name}")
}

/// The refusal a `DeleteCluster` of a cluster whose `DeletionProtection` is on
/// earns.
///
/// Deletion protection is a switch on the cluster ("Indicates whether deletion
/// protection is enabled for the cluster", `Cluster$deletionProtection`), so the
/// refusal states the cluster the request named and the switch that forbids the
/// call, by the member's own name: fixed text around the cluster name, which is the
/// only variable part. The caller's recovery — turn the switch off through
/// `UpdateClusterConfig` — follows from naming it, so the sentence does not spell
/// it out.
pub fn cluster_deletion_protected(cluster_name: &str) -> String {
    format!("Cluster {cluster_name} cannot be deleted while DeletionProtection is enabled.")
}

/// The refusal a partial EKS Auto Mode configuration earns.
///
/// EKS Auto Mode is ONE mode of the cluster made of three capabilities — compute,
/// block storage and load balancing — so the three `enabled` members are set to
/// the same value or the request is refused. Fixed text: it names the three
/// members, and WHICH of them is out of step is already in the caller's own
/// request.
pub const AUTO_MODE_ALL_OR_NOTHING: &str =
    "All of computeConfig.enabled, kubernetesNetworkConfig.elasticLoadBalancing.enabled, and \
     storageConfig.blockStorage.enabled must be set to either true or false.";

/// The refusal a compute capability that names node pools but no node role earns.
///
/// The role is what the instances of those pools run as — "The ARN of the IAM Role
/// EKS will assign to EC2 Managed Instances in your EKS Auto Mode cluster"
/// (`ComputeConfigRequest$nodeRoleArn`) — so a `nodePools` list without it names
/// pools nothing can run. Fixed text naming the two members, in the register of
/// [`AUTO_MODE_ALL_OR_NOTHING`] above.
pub const AUTO_MODE_NODE_ROLE_REQUIRED: &str =
    "computeConfig.nodeRoleArn must be specified when computeConfig.nodePools is specified.";

/// The refusal a `kubernetesNetworkConfig.serviceIpv4Cidr` earns, or `None` when
/// the block is one Amazon EKS assigns Kubernetes services from.
///
/// The three constraints are the member's own: it is not specified at all for an
/// `ipv6` cluster ("Don't specify a value if you select `ipv6` for `ipFamily`" —
/// "Kubernetes assigns services `IPv6` addresses from the unique local address
/// range"), it sits within one of the RFC 1918 blocks
/// ([`SERVICE_IPV4_PRIVATE_BLOCKS`]), and its netmask is between
/// [`SERVICE_IPV4_MIN_PREFIX`] and [`SERVICE_IPV4_MAX_PREFIX`] inclusive. The
/// cluster is named in the sentence because the block is a property of THAT
/// cluster's network, which "you can't change after the cluster is created".
pub fn service_ipv4_cidr_refusal(cluster_name: &str, cidr: &str, ip_family: &str) -> Option<String> {
    if ip_family.eq_ignore_ascii_case(IP_FAMILY_IPV6) {
        return Some(format!(
            "serviceIpv4Cidr cannot be specified when creating an \"{IP_FAMILY_IPV6}\" cluster"
        ));
    }
    let Ok(block) = ::ipnet::Ipv4Net::from_str(cidr) else {
        return Some(invalid_service_cidr(
            cluster_name,
            cidr,
            "must be a valid IPv4 CIDR block",
        ));
    };
    // The block the caller named has to be CONTAINED in a private range, not
    // merely overlap one: an address of the service CIDR that is not private is
    // not an address Kubernetes can assign a service.
    let private = SERVICE_IPV4_PRIVATE_BLOCKS
        .iter()
        .filter_map(|range| ::ipnet::Ipv4Net::from_str(range).ok())
        .any(|range| range.contains(&block));
    if !private {
        return Some(invalid_service_cidr(
            cluster_name,
            cidr,
            "must be within RFC1918 range",
        ));
    }
    if block.prefix_len() < SERVICE_IPV4_MIN_PREFIX || block.prefix_len() > SERVICE_IPV4_MAX_PREFIX {
        return Some(invalid_service_cidr(
            cluster_name,
            cidr,
            &format!(
                "must have netmask size between /{SERVICE_IPV4_MIN_PREFIX} and \
                 /{SERVICE_IPV4_MAX_PREFIX} inclusive"
            ),
        ));
    }
    None
}

/// The refusal a service CIDR earns for overlapping the network the cluster is
/// created in, or `None` when it overlaps none of `vpc_cidr_blocks`.
///
/// "Doesn't overlap with any CIDR block assigned to the VPC that you selected"
/// (`KubernetesNetworkConfigRequest$serviceIpv4Cidr`): pod addresses come from the
/// subnets' own ranges, so a service range inside the VPC's would collide with
/// them. The VPC range reported is the block that actually overlaps, read from the
/// VPC the request's subnets resolved to.
pub fn service_ipv4_cidr_vpc_overlap_refusal(
    cluster_name: &str,
    cidr: &str,
    vpc_cidr_blocks: &[String],
) -> Option<String> {
    let block = ::ipnet::Ipv4Net::from_str(cidr).ok()?;
    let overlapping = vpc_cidr_blocks.iter().find(|vpc_cidr| {
        // Two CIDR blocks are aligned prefixes, so they overlap exactly when one
        // contains the other — which `ipnet` answers for a whole block.
        ::ipnet::Ipv4Net::from_str(vpc_cidr)
            .map(|vpc_block| vpc_block.contains(&block) || block.contains(&vpc_block))
            .unwrap_or(false)
    })?;
    Some(invalid_service_cidr(
        cluster_name,
        cidr,
        &format!("overlaps with VPC range {overlapping}"),
    ))
}

/// The one sentence every `serviceIpv4Cidr` refusal is worded with: the block the
/// request named, the cluster it was named for, and the constraint it broke.
fn invalid_service_cidr(cluster_name: &str, cidr: &str, why: &str) -> String {
    format!("Invalid Cluster Service Ipv4 CIDR {cidr} for the cluster {cluster_name}, {why}")
}

/// The three capabilities that make a cluster an EKS Auto Mode cluster, as one
/// request asked for them.
///
/// Each is a tri-state: the request may enable it, disable it, or say nothing —
/// which is why these are `Option<bool>` rather than booleans. A capability the
/// request is silent about is not enabled.
#[derive(Debug, Clone, Copy, Default)]
pub struct AutoModeRequest {
    /// `computeConfig.enabled`.
    pub compute: Option<bool>,
    /// `storageConfig.blockStorage.enabled`.
    pub block_storage: Option<bool>,
    /// `kubernetesNetworkConfig.elasticLoadBalancing.enabled`.
    pub load_balancing: Option<bool>,
    /// Whether `computeConfig.nodePools` names at least one node pool.
    pub has_node_pools: bool,
    /// Whether `computeConfig.nodeRoleArn` was given.
    pub has_node_role_arn: bool,
}

impl AutoModeRequest {
    /// The three capabilities as ONE `CreateCluster`-shaped request asked for them.
    pub fn from_request(
        compute: Option<&ComputeConfigRequest>,
        storage: Option<&StorageConfigRequest>,
        network: Option<&KubernetesNetworkConfigRequest>,
    ) -> Self {
        Self {
            compute: compute.and_then(|config| config.enabled),
            block_storage: storage
                .and_then(|config| config.block_storage.as_ref())
                .and_then(|block_storage| block_storage.enabled),
            load_balancing: network
                .and_then(|config| config.elastic_load_balancing.as_ref())
                .and_then(|load_balancing| load_balancing.enabled),
            has_node_pools: compute
                .and_then(|config| config.node_pools.as_ref())
                .is_some_and(|pools| !pools.is_empty()),
            has_node_role_arn: compute
                .and_then(|config| config.node_role_arn.as_ref())
                .is_some_and(|role_arn| !role_arn.trim().is_empty()),
        }
    }

    /// The three capabilities as a cluster REPORTS them — what an update is judged
    /// on, where the values are the ones it would leave the cluster with.
    pub fn from_reported(
        compute: Option<&ComputeConfigResponse>,
        storage: Option<&StorageConfigResponse>,
        network: Option<&KubernetesNetworkConfigResponse>,
    ) -> Self {
        Self {
            compute: compute.and_then(|config| config.enabled),
            block_storage: storage
                .and_then(|config| config.block_storage.as_ref())
                .and_then(|block_storage| block_storage.enabled),
            load_balancing: network
                .and_then(|config| config.elastic_load_balancing.as_ref())
                .and_then(|load_balancing| load_balancing.enabled),
            has_node_pools: compute
                .and_then(|config| config.node_pools.as_ref())
                .is_some_and(|pools| !pools.is_empty()),
            has_node_role_arn: compute
                .and_then(|config| config.node_role_arn.as_ref())
                .is_some_and(|role_arn| !role_arn.trim().is_empty()),
        }
    }

    /// The refusal this combination earns, or `None` when the three capabilities
    /// agree and the compute capability carries what its node pools need.
    ///
    /// EKS Auto Mode is ONE mode of the cluster made of three capabilities —
    /// compute ("EKS Auto Mode will create and delete EC2 Managed Instances"),
    /// block storage ("will create and delete EBS volumes") and load balancing
    /// ("will create and delete load balancers") — so they are enabled or
    /// disabled TOGETHER: a cluster with compute managed by Auto Mode but storage
    /// or load balancing managed by the caller is not a configuration the mode
    /// has. A compute capability that names node pools also names the role the
    /// instances of those pools run as ("The ARN of the IAM Role EKS will assign
    /// to EC2 Managed Instances in your EKS Auto Mode cluster",
    /// `ComputeConfigRequest$nodeRoleArn`).
    pub fn refusal(&self) -> Option<String> {
        let compute = self.compute.unwrap_or(false);
        let block_storage = self.block_storage.unwrap_or(false);
        let load_balancing = self.load_balancing.unwrap_or(false);
        if compute != block_storage || compute != load_balancing {
            return Some(AUTO_MODE_ALL_OR_NOTHING.to_string());
        }
        if self.has_node_pools && !self.has_node_role_arn {
            return Some(AUTO_MODE_NODE_ROLE_REQUIRED.to_string());
        }
        None
    }
}

/// The refusal a requested `supportType` earns for the version it is asked for, or
/// `None` when that version has the support the request accepts.
///
/// `support` is where the version sits in the Amazon EKS support lifecycle at this
/// instant ([`ClusterVersionRecord::support_status`]). A version that is past
/// standard support is still created, but only by a request that accepts extended
/// support: "If the cluster is set to `STANDARD`, it will be automatically upgraded
/// at the end of standard support" (`UpgradePolicyRequest$supportType`), and there
/// is no standard support left to upgrade it at the end of. Whether the version is
/// offered at all is [`super::offers_cluster_version`]'s question, not this one's.
pub fn extended_support_refusal(
    version: &str,
    support: SupportStatus,
    support_type: &str,
) -> Option<String> {
    if support == SupportStatus::StandardSupport || support_type != SUPPORT_TYPE_STANDARD {
        return None;
    }
    Some(format!(
        "The requested Kubernetes version \"{version}\" is only supported under extended \
         support. Please change the supportType to extended support to create the cluster."
    ))
}

/// The refusal a `username` earns on an access entry whose `type` Amazon EKS
/// manages the username of, or `None` when the entry's type is one a caller names
/// the username for.
///
/// Only a `STANDARD` access entry authenticates as a username the caller chooses:
/// for every other type the entry stands for a node or a pod Amazon EKS admits
/// itself, and the username is the one it templates for that type
/// ([`default_username`]) — "We recommend not specifying a username and letting
/// Amazon EKS specify it for you... or constraints before specifying your own
/// username" (`CreateAccessEntryRequest$username`). The type is the only variable
/// part of the sentence.
pub fn username_refusal(entry_type: &str) -> Option<String> {
    if entry_type == ACCESS_ENTRY_TYPE_STANDARD {
        return None;
    }
    Some(format!(
        "The specified username is invalid: setting username is not allowed when the type is \
         \"{entry_type}\"."
    ))
}
