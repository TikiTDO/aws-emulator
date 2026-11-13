// SPDX-License-Identifier: CC0-1.0

//! The EC2 network a request PLACES an elastic network interface in: a fact this service
//! resolves rather than holds.
//!
//! Every member of this API that places a MediaConnect-managed elastic network interface in
//! a client VPC names the subnet it goes in and the security groups it carries — the router
//! network interface's `VpcRouterNetworkInterfaceConfiguration` ("the ID of the subnet
//! within the VPC to associate the router network interface with" / "the IDs of the security
//! groups to associate with the router network interface within the VPC") and a flow's
//! `VpcInterfaceRequest`, whose `NetworkInterfaceIds` are documented as "IDs of the network
//! interfaces created in customer's account by MediaConnect". A subnet and a security group
//! are not resources of this service: they are Amazon EC2's, so what a request names is
//! resolved against EC2, reached the way any emulated service reaches another — through the
//! in-process callback ([`smithy_server_runtime::OperationContext::callback`]), so this
//! crate names no other service's crate.
//!
//! Every refusal built here is grounded in what EC2 ANSWERED. An emulator with no EC2
//! mounted knows nothing about the account's network, and nothing is what it refuses on: a
//! refusal is never assumed from silence. This service only GATES on the placement — it
//! keeps the ids the request carried and reports no VPC or Availability Zone of its own —
//! so there is nothing it could not report for a hop it could not make.
//!
//! NOTHING here reaches the network. The callback is the framework's in-process dispatcher:
//! it hands the request to a service registered in this same process, so no socket is
//! opened, no address from the request is contacted, and the only destination involved is
//! this emulator itself. The Region label the request carries selects which of this
//! emulator's own regional state answers, not a host.
//!
//! What the caller is told is this service's own refusal detail: the member at fault, and
//! what the read this service made on the caller's behalf came back with — that EC2 reported
//! no such resource for the id the request named, or, where the read itself was refused, the
//! code and the sentence EC2 answered. The detail goes inside the envelope every refusal
//! about a request's content carries ([`super::validation_failed_message`]), because the
//! error channel is per operation while the rule is one.

/// An AWS SDK client for EC2 over the in-process callback into this same emulator.
///
/// The callback is the framework's documented seam for calling another emulated service: it
/// dispatches IN PROCESS to a service registered in this very emulator, so no socket is
/// opened and nothing leaves the host. The endpoint URL is only the address the SDK needs to
/// name the service and Region it is addressing — the callback's HTTP client never resolves
/// or connects to it — which is also why there is nothing to retry, time out, watch for a
/// stalled stream, or cache an identity for.
fn ec2_client(
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

/// The prefix of every EC2 error code that says a value names no subnet of the account:
/// `InvalidSubnetID.NotFound` for an id no subnet carries, `InvalidSubnetID.Malformed` for a
/// value that is not a subnet id at all. Both say the same thing about a placement.
const SUBNET_REFUSAL_PREFIX: &str = "InvalidSubnetID.";

/// The EC2 error codes that say a value names no security group of the account:
/// `InvalidGroup.NotFound` for an id no group carries, `InvalidGroupId.Malformed` for a
/// value that is not a group id at all. They are named in full, because
/// `InvalidGroupName.*` and the other `InvalidGroup…` codes say something else.
const GROUP_REFUSAL_CODES: [&str; 2] = ["InvalidGroup.NotFound", "InvalidGroupId.Malformed"];

/// What a placement member names, as the refusal calls the resource it must name.
const SUBNET: &str = "subnet";

/// See [`SUBNET`].
const SECURITY_GROUP: &str = "security group";

/// How many security groups ONE placement may name, which is also how many this service
/// resolves.
///
/// The list comes from the request and is both cloned into the resolving call below and
/// walked against what EC2 answered, so without a ceiling one request would size that work.
/// `VpcRouterNetworkInterfaceConfiguration$SecurityGroupIds` carries a modelled `length`
/// 1..5, so the bound only ever decides a flow's `VpcInterfaceRequest$SecurityGroupIds`,
/// which the model bounds not at all: it is therefore the ceiling this crate already puts on
/// what one create request may store in a flow's collections
/// ([`super::MAX_FLOW_COLLECTION_ENTRIES`]), rather than a second number.
pub const MAX_PLACEMENT_SECURITY_GROUPS: usize = super::MAX_FLOW_COLLECTION_ENTRIES;

/// How many placements ONE request may have resolved, which bounds the fan-out of
/// [`unresolved_vpc_interface_placements`]: one entry of a caller-sized list costs one
/// resolution, so the count is capped before the first entry is resolved. It is the same
/// ceiling this crate already puts on what one create request may store in a flow's
/// collections ([`super::MAX_FLOW_COLLECTION_ENTRIES`]), and no tighter than the
/// per-operation bounds that run before it, so it refuses nothing they admit.
pub const MAX_RESOLVED_PLACEMENTS: usize = super::MAX_FLOW_COLLECTION_ENTRIES;

/// Why the EC2 network the placement `member` describes cannot be placed in, as the detail
/// the refusal carries — or `None` when EC2 resolved the subnet and every security group it
/// names, which includes the case where this emulator has no EC2 to ask.
///
/// `member` is the member path the placement arrived under (`Configuration.Vpc`,
/// `VpcInterfaces[0]`), so the refusal names what the caller sent; `region` is the Region
/// the interface is placed in, which selects which of this emulator's own regional state
/// answers. Each caller reads it as a Region LABEL before calling, and it is read as one
/// again here, because this is where it reaches the address the SDK names the peer by: a
/// value carrying a host or path delimiter would name an authority the caller chose rather
/// than the service this emulator asks, so it is reported as no Region at all and nothing
/// is refused on it.
///
/// Three questions, in this order. Whether the group list is within
/// [`MAX_PLACEMENT_SECURITY_GROUPS`] and whether either member is EMPTY are answered from
/// the request ALONE — the ceiling first, so it bounds everything below it, and both hold
/// whether or not there is an EC2 to ask — and then whether EC2 resolves the subnet and
/// every group id.
pub async fn unresolved_placement(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    member: &str,
    subnet_id: &str,
    security_group_ids: &[String],
) -> Option<String> {
    if security_group_ids.len() > MAX_PLACEMENT_SECURITY_GROUPS {
        return Some(format!(
            "{member}.SecurityGroupIds may name at most {MAX_PLACEMENT_SECURITY_GROUPS} \
             security groups; this one names {}.",
            security_group_ids.len()
        ));
    }
    // A subnet id and a security group id are values EC2 MINTED, and it mints none that is
    // empty, so an empty member names no resource of any account: that is answerable from
    // the request alone, which is why it is refused here rather than left to a read that
    // would have to be made to find it out. Both members are `required`, and the router
    // network interface's `SecurityGroupIds` carries a modelled `length` 1..5 — the lower
    // bound a flow's own `VpcInterfaces` entry gets from this rule instead, the model
    // bounding that list not at all.
    if subnet_id.is_empty() {
        return Some(format!(
            "{member}.SubnetId names the subnet the network interface is created in, so it \
             must name a subnet of this account; this request leaves it empty."
        ));
    }
    if security_group_ids.is_empty() {
        return Some(format!(
            "{member}.SecurityGroupIds names the security groups the network interface is \
             created under, so it must name at least one security group of this account; \
             this request names none."
        ));
    }
    // No Region label, so no in-process peer to address: the same answer an EC2 that is not
    // mounted gets, which grounds no refusal.
    let region = ::service_common::arn::region_label(region)?;
    let ec2 = ec2_client(ctx, region);
    if let Some(refusal) = unresolved_subnet(&ec2, member, subnet_id).await {
        return Some(refusal);
    }
    unresolved_security_groups(&ec2, member, security_group_ids).await
}

/// Why the subnet `subnet_id` names cannot be placed in, by the rule
/// [`unresolved_placement`] states. `subnet_id` is non-empty: an empty member is refused
/// from the request alone, before anything is asked.
async fn unresolved_subnet(
    ec2: &::aws_sdk_ec2::Client,
    member: &str,
    subnet_id: &str,
) -> Option<String> {
    let answer = ec2
        .describe_subnets()
        .subnet_ids(subnet_id.to_string())
        .send()
        .await;
    let error = match answer {
        Ok(output) => {
            // EC2 reports one subnet per id it resolved, so a read that reports none for the
            // id the request named says the same thing a refusal does, about the same id.
            if output
                .subnets()
                .iter()
                .any(|subnet| subnet.subnet_id() == Some(subnet_id))
            {
                return None;
            }
            return Some(absent_detail(
                &format!("{member}.SubnetId"),
                SUBNET,
                subnet_id,
            ));
        }
        Err(error) => error,
    };
    relayed_refusal(
        &error,
        &format!("{member}.SubnetId"),
        &|code| code.starts_with(SUBNET_REFUSAL_PREFIX),
        "the subnet a request named could not be resolved against EC2",
    )
}

/// Why the security groups `security_group_ids` names cannot be carried, by the rule
/// [`unresolved_placement`] states. The list is non-empty and within
/// [`MAX_PLACEMENT_SECURITY_GROUPS`]: both are answered from the request alone, before
/// anything is asked.
async fn unresolved_security_groups(
    ec2: &::aws_sdk_ec2::Client,
    member: &str,
    security_group_ids: &[String],
) -> Option<String> {
    let answer = ec2
        .describe_security_groups()
        .set_group_ids(Some(security_group_ids.to_vec()))
        .send()
        .await;
    let error = match answer {
        Ok(output) => {
            let reported: Vec<&str> = output
                .security_groups()
                .iter()
                .filter_map(::aws_sdk_ec2::types::SecurityGroup::group_id)
                .collect();
            let absent = security_group_ids
                .iter()
                .find(|id| !reported.contains(&id.as_str()))?;
            return Some(absent_detail(
                &format!("{member}.SecurityGroupIds"),
                SECURITY_GROUP,
                absent,
            ));
        }
        Err(error) => error,
    };
    relayed_refusal(
        &error,
        &format!("{member}.SecurityGroupIds"),
        &|code| GROUP_REFUSAL_CODES.contains(&code),
        "the security groups a request named could not be resolved against EC2",
    )
}

/// The refusal detail for a placement member whose id the read RESOLVED and reported no
/// resource for: the member at fault, what it must name, and the id the request carried.
///
/// This is the service's own sentence, because EC2 answered the read rather than refusing
/// it: it reported no such resource, which says the same thing about the placement as a
/// refusal would, but it worded no refusal to relay.
fn absent_detail(member: &str, resource: &str, id: &str) -> String {
    format!(
        "{member} names no {resource} of this account: Amazon EC2 reports none for {id:?}."
    )
}

/// The refusal detail for a placement member whose read EC2 REFUSED: the member at fault,
/// then the code and the sentence EC2 answered, which is what names the offending id.
fn relayed_detail(member: &str, code: &str, sentence: &str) -> String {
    format!("{member} names no resource of this account. Amazon EC2 answered {code}: {sentence}")
}

/// The detail an EC2 read that FAILED grounds: the refusal to relay when EC2 reported that
/// the id names no such resource — which `is_a_refusal` decides from the code it answered —
/// and nothing otherwise.
///
/// Anything else is this emulator failing to ASK rather than EC2 reporting a bad id (there
/// is no EC2 mounted, or the answer could not be read), and nothing is what a request is
/// refused on then — worth a warning, because a deployment with no EC2 to ask is one where
/// this gate does not run.
fn relayed_refusal<
    E: ::aws_smithy_types::error::metadata::ProvideErrorMetadata + ::std::error::Error + 'static,
>(
    error: &::aws_smithy_runtime_api::client::result::SdkError<
        E,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
    member: &str,
    is_a_refusal: &dyn Fn(&str) -> bool,
    unresolved: &str,
) -> Option<String> {
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    let refused = error.as_service_error();
    let code = refused
        .and_then(ProvideErrorMetadata::code)
        .unwrap_or_default()
        .to_string();
    if !is_a_refusal(&code) {
        ::tracing::warn!(
            error = %::aws_smithy_types::error::display::DisplayErrorContext(error),
            "{unresolved}"
        );
        return None;
    }
    Some(relayed_detail(
        member,
        &code,
        // EC2 names the offending id in its own sentence, so that sentence is what is
        // relayed. A refusal it worded none for still states what it can: its code.
        refused
            .and_then(ProvideErrorMetadata::message)
            .unwrap_or(&code),
    ))
}

/// Why the EC2 network a router network interface's `Configuration` places it in cannot be
/// placed in, or `None` when it can — which includes a PUBLIC configuration, whose
/// `AllowRules` name no EC2 resource at all.
///
/// Both CreateRouterNetworkInterface and UpdateRouterNetworkInterface carry this union
/// ("the configuration settings for the router network interface"), so both resolve the
/// placement here and neither can admit what the other refuses.
pub async fn unresolved_router_network_interface_placement(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    configuration: &crate::ops::types::RouterNetworkInterfaceConfiguration,
) -> Option<String> {
    let crate::ops::types::RouterNetworkInterfaceConfiguration::Vpc(vpc) = configuration else {
        return None;
    };
    unresolved_placement(
        ctx,
        region,
        "Configuration.Vpc",
        &vpc.subnet_id,
        &vpc.security_group_ids,
    )
    .await
}

/// Why the EC2 network the VPC interfaces `requests` would be created in cannot be placed
/// in, or `None` when every one of them resolves.
///
/// Both CreateFlow (`VpcInterfaces`) and AddFlowVpcInterfaces add VPC interfaces to a flow,
/// and each one is an elastic network interface MediaConnect creates in the caller's
/// account, so both resolve every entry here — one member path per entry, as the rest of
/// this crate names a nested request member.
///
/// The list is CALLER-SIZED and this resolves one placement per entry, so the count is
/// bounded here too, before the first entry is resolved: each operation bounds its own
/// member before it calls (CreateFlow at [`super::MAX_FLOW_COLLECTION_ENTRIES`],
/// AddFlowVpcInterfaces at [`super::MAX_FLOW_VPC_INTERFACES`]), and this is the same
/// ceiling applied in the shared path, so no caller of it can drive an unbounded fan-out.
pub async fn unresolved_vpc_interface_placements(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    requests: &[crate::ops::types::VpcInterfaceRequest],
) -> Option<String> {
    if requests.len() > MAX_RESOLVED_PLACEMENTS {
        return Some(format!(
            "VpcInterfaces may name at most {MAX_RESOLVED_PLACEMENTS} VPC interfaces; this \
             request names {}.",
            requests.len()
        ));
    }
    for (index, request) in requests.iter().enumerate() {
        if let Some(refusal) = unresolved_placement(
            ctx,
            region,
            &format!("VpcInterfaces[{index}]"),
            &request.subnet_id,
            &request.security_group_ids,
        )
        .await
        {
            return Some(refusal);
        }
    }
    None
}
