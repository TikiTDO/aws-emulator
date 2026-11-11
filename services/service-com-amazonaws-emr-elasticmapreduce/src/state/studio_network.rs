// SPDX-License-Identifier: CC0-1.0

//! The resources an EMR Studio is WIRED TO, which are not this service's.
//!
//! `CreateStudio` names an Amazon EC2 VPC, the subnets the Studio's Workspaces run in, the
//! two security groups they carry, and the IAM role the Studio assumes. None of those is a
//! resource of this service: the network objects live in Amazon EC2 and the role in IAM, and
//! they are resolved THERE — over the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), so this crate names no other
//! service's crate. `service_common::network` states the contract this follows: Amazon EC2
//! is the ONE answer to which VPCs, subnets and security groups the account has.
//!
//! Every refusal built here is grounded in what the other service ANSWERED. An emulator
//! with no EC2 or no IAM mounted knows nothing about the network or the role, and nothing is
//! what it refuses on: a Studio GATES on existence and keeps nothing of the network, so
//! silence costs the create nothing to skip, and a refusal assumed from it would be
//! ungrounded.

use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

/// The AWS SDK for EC2 driven over the in-process callback into this same emulator.
///
/// The callback is the framework's seam for calling another emulated service: it routes in
/// process, so no network client is configured here. There is nothing to retry, time out or
/// watch for a stalled stream — each of those would need a sleep implementation this client
/// is not given — and the credentials the callback hands over are used directly rather than
/// cached.
pub(super) fn ec2_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_ec2::Client {
    let config = ::aws_sdk_ec2::Config::builder()
        .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_ec2::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://ec2.{region}.amazonaws.com"))
        .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_ec2::Client::from_conf(config)
}

/// The prefix of every Amazon EC2 error code that says the account holds no VPC under an
/// identifier a read was given, whether because none carries it or because it is no VPC id
/// at all.
const INVALID_VPC_ID: &str = "InvalidVpcID.";

/// The same for a subnet identifier.
const INVALID_SUBNET_ID: &str = "InvalidSubnetID.";

/// The same for a security group identifier.
const INVALID_GROUP: &str = "InvalidGroup.";

/// The service principal an EMR Studio's `ServiceRole` has to trust: the Studio "assumes"
/// that role (`CreateStudio$ServiceRole` doc), and what assumes it is Amazon EMR, so the
/// trust policy on the role has to admit the Amazon EMR service principal.
const STUDIO_SERVICE_PRINCIPAL: &str = "elasticmapreduce.amazonaws.com";

/// What a `VpcId` that names no VPC of this account is refused with.
pub fn absent_vpc_message(vpc_id: &str) -> String {
    format!("VpcId '{vpc_id}' does not name a VPC in this account")
}

/// What a `SubnetIds` entry that names no subnet of this account is refused with.
pub fn absent_subnet_message(subnet_id: &str) -> String {
    format!("SubnetIds entry '{subnet_id}' does not name a subnet in this account")
}

/// What a security-group member that names no security group of this account is refused
/// with: the refusal names the MEMBER, because a Studio carries two of them.
pub fn absent_security_group_message(member: &str, group_id: &str) -> String {
    format!("{member} '{group_id}' does not name a security group in this account")
}

/// What a `ServiceRole` the Studio could not assume is refused with.
///
/// Recorded behavior for the text: the EMR Studio backend answers the same wrapped form
/// every refusal it raises carries ([`super::unknown_studio_message`]) — its own service
/// name and the wire fields of the error it raised, with this response's own request id
/// ([`service_common::query_error_envelope::new_request_id`]) — in front of which it states
/// that it has no permission to assume the role. The one value taken from the request is the
/// role it was handed, which is what a caller has to fix.
pub fn unassumable_service_role_message(service_role: &str) -> String {
    let backend = super::STUDIO_BACKEND_SERVICE;
    format!(
        "This entity does not have permissions to assume role '{service_role}' (Service: \
         {backend}; Status Code: 400; Error Code: InvalidRequestException; Request ID: {}; \
         Proxy: null)",
        ::service_common::query_error_envelope::new_request_id()
    )
}

/// The refusal the `VpcId` a Studio names earns, or `None` when Amazon EC2 reported that
/// VPC — and also when the hop could not be made, for the reason this module's docs give.
pub async fn unresolved_vpc(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    vpc_id: &str,
) -> Option<String> {
    let answer = ec2_client(ctx, region)
        .describe_vpcs()
        .vpc_ids(vpc_id)
        .send()
        .await;
    let absent = match &answer {
        // A read that resolved and reports no VPC under the id says the same thing the
        // refusal below does, about the same id.
        Ok(output) => !output.vpcs().iter().any(|vpc| vpc.vpc_id() == Some(vpc_id)),
        Err(error) => answered_absent(error.code(), INVALID_VPC_ID, "VPC", vpc_id),
    };
    absent.then(|| absent_vpc_message(vpc_id))
}

/// The refusal the `SubnetIds` of a Studio earn, by the rule [`unresolved_vpc`] states:
/// the FIRST entry Amazon EC2 reported no subnet for.
///
/// Each entry is resolved on its own, so what the refusal names is the entry EC2 answered
/// about — a read batching every entry is refused as a whole the moment one of them does
/// not resolve, which says nothing about WHICH.
pub async fn unresolved_subnets(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subnet_ids: &[String],
) -> Option<String> {
    let client = ec2_client(ctx, region);
    for subnet_id in subnet_ids {
        let answer = client
            .describe_subnets()
            .subnet_ids(subnet_id)
            .send()
            .await;
        let absent = match &answer {
            Ok(output) => !output
                .subnets()
                .iter()
                .any(|subnet| subnet.subnet_id() == Some(subnet_id.as_str())),
            Err(error) => answered_absent(error.code(), INVALID_SUBNET_ID, "subnet", subnet_id),
        };
        if absent {
            return Some(absent_subnet_message(subnet_id));
        }
    }
    None
}

/// The refusal the security groups a Studio names earn, by the rule [`unresolved_vpc`]
/// states, each resolved on its own for the reason [`unresolved_subnets`] gives. Each entry
/// is the member name the refusal has to state and the group id that member carried.
pub async fn unresolved_security_groups(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    groups: &[(&str, &str)],
) -> Option<String> {
    let client = ec2_client(ctx, region);
    for (member, group_id) in groups {
        let answer = client
            .describe_security_groups()
            .group_ids(*group_id)
            .send()
            .await;
        let absent = match &answer {
            Ok(output) => !output
                .security_groups()
                .iter()
                .any(|group| group.group_id() == Some(*group_id)),
            Err(error) => {
                answered_absent(error.code(), INVALID_GROUP, "security group", group_id)
            }
        };
        if absent {
            return Some(absent_security_group_message(member, group_id));
        }
    }
    None
}

/// The refusal the `ServiceRole` a Studio names earns, or `None` when IAM proved the
/// Studio may assume it — and also when there is no IAM to ask, for the reason this
/// module's docs give.
///
/// The role is the one the Studio RUNS AS, so the question is the tree's one reading of it
/// ([`service_common::iam_role::role_is_unassumable`]): IAM holds a role of this account
/// under the name the value carries, and the trust policy that role carries admits Amazon
/// EMR.
pub async fn unassumable_service_role(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    service_role: &str,
) -> Option<String> {
    ::service_common::iam_role::role_is_unassumable(
        ctx,
        region,
        service_role,
        STUDIO_SERVICE_PRINCIPAL,
    )
    .await
    .then(|| unassumable_service_role_message(service_role))
}

/// Whether the `code` Amazon EC2 refused a read with is EC2 ANSWERING that the account
/// holds no such resource — every code under `prefix` says that — rather than this
/// emulator failing to ask it at all, which refuses nothing and is traced instead.
fn answered_absent(code: Option<&str>, prefix: &str, kind: &str, requested: &str) -> bool {
    let code = code.unwrap_or_default();
    if code.starts_with(prefix) {
        return true;
    }
    // The caller's own value is rendered in its escaped `Debug` form, so an identifier
    // carrying a newline cannot forge a log line.
    ::tracing::debug!(
        requested = ?requested,
        code = ?code,
        "the {kind} a Studio named could not be resolved against EC2"
    );
    false
}
