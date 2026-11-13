// SPDX-License-Identifier: CC0-1.0

//! The resources a network connector is WIRED TO, which are not this service's.
//!
//! A connector provisions elastic network interfaces in the caller's VPC, so what a
//! request names is an EC2 subnet, an EC2 security group and the IAM role Lambda assumes
//! to manage those interfaces. None of those is a resource of this service: they live in
//! EC2 and in IAM, and they are resolved there — over the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), so this crate names no other
//! service's crate.
//!
//! Every refusal built here is grounded in what the OTHER service ANSWERED. An emulator
//! with no EC2 or no IAM mounted knows nothing about the subnet, the group or the role,
//! and nothing is what it refuses on: a refusal is never assumed from silence.

use crate::ops::types::InvalidParameterValueException;

/// An AWS SDK client over the in-process callback into this same emulator.
///
/// The callback is the framework's documented seam for calling another emulated service:
/// it routes in process, so no network client is configured here. The transport is
/// in-process, so there is nothing to retry, time out or watch for a stalled stream — and
/// each of those would need a sleep implementation this client is not given. The same
/// goes for caching the identity: the credentials the callback hands over are used
/// directly.
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
        .stalled_stream_protection(
            ::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_ec2::Client::from_conf(config)
}

/// The EC2 read that resolves the subnets a `Configuration` names.
const DESCRIBE_SUBNETS: &str = "DescribeSubnets";

/// The EC2 read that resolves the security groups a `Configuration` names.
const DESCRIBE_SECURITY_GROUPS: &str = "DescribeSecurityGroups";

/// The EC2 error code for an identifier the account holds no subnet for, which is also
/// what a read that resolved WITHOUT reporting one of the requested ids ran into.
const INVALID_SUBNET_ID_NOT_FOUND: &str = "InvalidSubnetID.NotFound";

/// The EC2 error codes a security group reference earns, as EC2's error catalogue spells
/// them: `InvalidGroup.NotFound` for an identifier the account holds no security group for,
/// and `InvalidGroupId.Malformed` for a value that is not a group id at all. Unlike the
/// subnet codes, which share one prefix, EC2 words these two under different ones — and both
/// say the same thing about the request: the value names no security group to place an
/// interface in.
const GROUP_REFUSAL_PREFIXES: &[&str] = &["InvalidGroup.", "InvalidGroupId."];

/// The EC2 error code for an identifier the account holds no security group for.
const INVALID_GROUP_NOT_FOUND: &str = "InvalidGroup.NotFound";

/// The service principal that assumes a connector's `OperatorRole`: the role is "the IAM
/// role that Lambda assumes to manage elastic network interfaces in your VPC", so the
/// trust policy on it has to admit Lambda.
const LAMBDA_SERVICE_PRINCIPAL: &str = "lambda.amazonaws.com";

/// The EC2 actions the `OperatorRole` has to grant to be usable: `OperatorRole` is
/// documented as the role that manages the connector's elastic network interfaces, and
/// "this role must have permissions for ec2:CreateNetworkInterface,
/// ec2:DeleteNetworkInterface, and related describe operations". These two are the ones
/// the model names outright, so they are what a role is proved against.
const OPERATOR_ROLE_ACTIONS: &[&str] = &["ec2:CreateNetworkInterface", "ec2:DeleteNetworkInterface"];

/// What a request naming an EC2 resource the account does not hold is refused with: the
/// failure of the EC2 read this service made on the caller's behalf, reported with the
/// call it made, the error code EC2 answered and the sentence EC2 worded — which is what
/// names the offending identifier to the caller.
fn downstream_ec2_failure(
    call: &str,
    code: &str,
    message: &str,
) -> InvalidParameterValueException {
    crate::state::invalid_parameter(format!(
        "Error occurred while {call}. EC2 Error Code: {code}. EC2 Error Message: {message}"
    ))
}

/// The refusal the EC2 resources `configuration` names earn, or `None` when EC2 resolved
/// every one of them — and also when there is no EC2 to ask, because a refusal has to be
/// grounded in what EC2 reported rather than assumed from silence.
///
/// A connector's interfaces carry the security groups the configuration names and are
/// provisioned in the subnets it names, so both are resolved before any state is written:
/// an identifier the account holds no such resource for names nothing to provision in. The
/// SECURITY GROUPS are resolved first, which is the one the service reports when neither
/// set resolves.
pub async fn unresolved_reference(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    configuration: &crate::ops::types::NetworkConnectorConfiguration,
) -> Option<InvalidParameterValueException> {
    let crate::ops::types::NetworkConnectorConfiguration::VpcEgressConfiguration(vpc_egress) =
        configuration;
    let subnet_ids = vpc_egress.subnet_ids.as_deref().unwrap_or_default();
    let security_group_ids = vpc_egress.security_group_ids.as_deref().unwrap_or_default();
    if let Some(refusal) = unresolved_security_groups(ctx, region, security_group_ids).await {
        return Some(refusal);
    }
    unresolved_subnets(ctx, region, subnet_ids).await
}

/// The refusal the subnet identifiers `subnet_ids` earn, by the rule
/// [`unresolved_reference`] states.
async fn unresolved_subnets(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subnet_ids: &[String],
) -> Option<InvalidParameterValueException> {
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    if subnet_ids.is_empty() {
        return None;
    }
    let answer = ec2_client(ctx, region)
        .describe_subnets()
        .set_subnet_ids(Some(subnet_ids.to_vec()))
        .send()
        .await;
    let error = match answer {
        Ok(output) => {
            let reported: Vec<&str> = output
                .subnets()
                .iter()
                .filter_map(::aws_sdk_ec2::types::Subnet::subnet_id)
                .collect();
            // A read that resolved but reports no subnet for an id the request named says
            // the same thing the refusal below does, and about the same id.
            let absent = subnet_ids
                .iter()
                .find(|id| !reported.contains(&id.as_str()))?;
            return Some(downstream_ec2_failure(
                DESCRIBE_SUBNETS,
                INVALID_SUBNET_ID_NOT_FOUND,
                &format!("The subnet ID '{absent}' does not exist"),
            ));
        }
        Err(error) => error,
    };
    let refused = error.as_service_error();
    let code = refused
        .and_then(ProvideErrorMetadata::code)
        .unwrap_or_default()
        .to_string();
    // Every `InvalidSubnetID.…` code says the same thing about a placement: the value
    // names no subnet of the account, whether because none carries that id or because it
    // is not a subnet id at all.
    if !code.starts_with("InvalidSubnetID.") {
        ::tracing::debug!(
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "the subnets a request named could not be resolved against EC2"
        );
        return None;
    }
    Some(downstream_ec2_failure(
        DESCRIBE_SUBNETS,
        &code,
        &refused
            .and_then(ProvideErrorMetadata::message)
            .map_or_else(String::new, str::to_string),
    ))
}

/// The refusal the security group identifiers `security_group_ids` earn, by the rule
/// [`unresolved_reference`] states.
async fn unresolved_security_groups(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    security_group_ids: &[String],
) -> Option<InvalidParameterValueException> {
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    if security_group_ids.is_empty() {
        return None;
    }
    let answer = ec2_client(ctx, region)
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
            return Some(downstream_ec2_failure(
                DESCRIBE_SECURITY_GROUPS,
                INVALID_GROUP_NOT_FOUND,
                &format!("The security group '{absent}' does not exist"),
            ));
        }
        Err(error) => error,
    };
    let refused = error.as_service_error();
    let code = refused
        .and_then(ProvideErrorMetadata::code)
        .unwrap_or_default()
        .to_string();
    if !GROUP_REFUSAL_PREFIXES
        .iter()
        .any(|prefix| code.starts_with(prefix))
    {
        ::tracing::debug!(
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "the security groups a request named could not be resolved against EC2"
        );
        return None;
    }
    Some(downstream_ec2_failure(
        DESCRIBE_SECURITY_GROUPS,
        &code,
        &refused
            .and_then(ProvideErrorMetadata::message)
            .map_or_else(String::new, str::to_string),
    ))
}

/// What a role Lambda cannot assume is refused with: the service states that it could not
/// assume the role it was handed, and points the caller at the trust policy, which is the
/// one thing that decides it.
fn unassumable_operator_role() -> InvalidParameterValueException {
    crate::state::invalid_parameter(
        "The service is unable to assume the provided NetworkConnectorOperatorRole. \
         Please verify the trust policy on the role."
            .to_string(),
    )
}

/// What a role Lambda can assume but that grants none of the interface permissions is
/// refused with: the call Lambda would make on the caller's behalf is the EC2 one, so the
/// refusal states that the EC2 call was unauthorized and names the member at fault.
fn unauthorized_operator_role() -> InvalidParameterValueException {
    crate::state::invalid_parameter(
        "Encountered unauthorized operation while calling EC2 due to invalid \
         ConnectorOperatorRole permissions"
            .to_string(),
    )
}

/// The refusal the `OperatorRole` a request carries earns, or `None` when IAM proved it
/// usable — and also when there is no IAM to ask, for the reason
/// [`unresolved_reference`] gives.
///
/// The role is proved usable in the two stages the service proves it in, each a refusal of
/// its own: Lambda has to be able to ASSUME it (IAM holds the role and its trust policy
/// admits the Lambda service principal), and the role has to GRANT the interface
/// operations Lambda performs under it.
pub async fn unusable_operator_role(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operator_role: Option<&str>,
) -> Option<InvalidParameterValueException> {
    let role_arn = operator_role.filter(|role| !role.is_empty())?;
    if ::service_common::iam_role::role_is_unassumable(
        ctx,
        region,
        role_arn,
        LAMBDA_SERVICE_PRINCIPAL,
    )
    .await
    {
        return Some(unassumable_operator_role());
    }
    // The permissions in force on the role are what IAM holds for it; a role whose
    // policies this could not READ grounds no refusal, so only an enumeration IAM
    // ANSWERED decides this.
    let documents = role_identity_documents(ctx, region, role_arn).await?;
    // The interfaces are provisioned in the caller's VPC, so what the role must grant is
    // the interface actions on the account's own network interfaces.
    let resource = ::engine::emulator_arn("ec2", region, "network-interface/*");
    let granted = OPERATOR_ROLE_ACTIONS.iter().all(|action| {
        ::service_common::iam_authorization::identity_decision(
            &documents,
            &::service_common::iam_authorization::Request::new(action, &resource, &[]),
        ) == ::service_common::iam_authorization::Decision::Allowed
    });
    (!granted).then(unauthorized_operator_role)
}

/// Every identity-based policy document IAM holds for the role `role_arn` names — the
/// documents of the managed policies attached to it, and its inline policies — or `None`
/// when this emulator could not read them at all (there is no IAM to ask, the value names
/// no role of this account, or an answer could not be read), which grounds no refusal.
///
/// An enumeration that succeeds and reports NO policy is a role that carries none, which
/// is an answer rather than a silence: it is reported as the empty set.
async fn role_identity_documents(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
) -> Option<Vec<String>> {
    let role_name = ::service_common::iam_role::role_name(role_arn)?;
    let iam = ::service_common::iam_client::iam_client(ctx, region);
    let mut documents = Vec::new();

    let attached = iam
        .list_attached_role_policies()
        .role_name(role_name)
        .send()
        .await
        .ok()?;
    for policy in attached.attached_policies() {
        let policy_arn = policy.policy_arn()?;
        let described = iam.get_policy().policy_arn(policy_arn).send().await.ok()?;
        let version_id = described.policy()?.default_version_id()?;
        let version = iam
            .get_policy_version()
            .policy_arn(policy_arn)
            .version_id(version_id)
            .send()
            .await
            .ok()?;
        documents.push(version.policy_version()?.document()?.to_string());
    }

    let inline = iam.list_role_policies().role_name(role_name).send().await.ok()?;
    for policy_name in inline.policy_names() {
        let policy = iam
            .get_role_policy()
            .role_name(role_name)
            .policy_name(policy_name)
            .send()
            .await
            .ok()?;
        documents.push(policy.policy_document().to_string());
    }

    Some(documents)
}
