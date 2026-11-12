// SPDX-License-Identifier: CC0-1.0

//! The EC2 resources a project's `vpcConfig` names, resolved against the service that
//! OWNS them.
//!
//! A `vpcConfig` attaches a project to a network: its `vpcId`, its `subnetIds` and its
//! `securityGroupIds` all name Amazon EC2 resources, none of which is Device Farm's. So
//! whether they exist is EC2's to answer, and the EC2 service mounted in THIS emulator is
//! asked — `DescribeSubnets`, `DescribeSecurityGroups` and `DescribeVpcs` over the
//! in-process callback ([`smithy_server_runtime::OperationContext::callback`]), so this
//! crate names no other service's crate — BEFORE any project state is written.
//!
//! NOTHING here reaches the network. The callback is the framework's in-process dispatcher:
//! it hands the request to a service registered in this same process, so no socket is
//! opened, no address from the request is contacted, and the only destination involved is
//! this emulator itself. The Region label the caller signed for selects which of this
//! emulator's own regional state answers, not a host.
//!
//! What a refusal SAYS is what that EC2 service answered: Device Farm makes the read on the
//! caller's behalf, so the sentence it worded (the one that names the offending id) is
//! relayed whole, followed by the context of the call it came from — the service that
//! answered, the status and error code it answered with, and that call's own request id.
//! Every part of it is this emulator's own text or a value from the caller's own request.
//!
//! Device Farm only GATES on existence here: it keeps nothing of the subnet, the group or
//! the VPC beyond the ids the request carried. So an emulator with no EC2 mounted learns
//! nothing to refuse a request on, and refuses nothing: a refusal is grounded in what EC2
//! reported, never assumed from silence.

use crate::ops::types::VpcConfig;

/// An AWS SDK client for EC2 over the in-process callback into this same emulator.
///
/// The callback is the framework's documented seam for calling another emulated service:
/// it dispatches IN PROCESS to a service registered in this very emulator, so no socket is
/// opened and nothing leaves the host. The endpoint URL below is only the address the SDK
/// needs to name the service and Region it is addressing — the callback's HTTP client never
/// resolves or connects to it — which is also why there is nothing to retry, time out, watch
/// for a stalled stream, or cache an identity for.
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

/// The service a relayed refusal names as the one that answered it.
const EC2_SERVICE_NAME: &str = "AmazonEC2";

/// The prefix of every EC2 error code that says a value names no subnet of the account:
/// `InvalidSubnetID.NotFound` for an id no subnet carries, `InvalidSubnetID.Malformed` for
/// a value that is not a subnet id at all. Both say the same thing about a placement.
const SUBNET_REFUSAL_PREFIX: &str = "InvalidSubnetID.";

/// The EC2 code for a subnet id the account holds no subnet for, which is also what a read
/// that resolved WITHOUT reporting one of the requested ids ran into.
const SUBNET_NOT_FOUND: &str = "InvalidSubnetID.NotFound";

/// The EC2 error codes that say a value names no security group of the account:
/// `InvalidGroup.NotFound` for an id no group carries, `InvalidGroupId.Malformed` for a
/// value that is not a group id at all. Both say the same thing about an attachment, and
/// they are named in full — `InvalidGroupName.*` and the other `InvalidGroup…` codes say
/// something else.
const GROUP_REFUSAL_CODES: &[&str] = &["InvalidGroup.NotFound", "InvalidGroupId.Malformed"];

/// The EC2 code for a security group id the account holds no group for.
const GROUP_NOT_FOUND: &str = "InvalidGroup.NotFound";

/// The prefix of every EC2 error code that says a value names no VPC of the account:
/// `InvalidVpcID.NotFound` and `InvalidVpcID.Malformed`.
const VPC_REFUSAL_PREFIX: &str = "InvalidVpcID.";

/// The EC2 code for a VPC id the account holds no VPC for.
const VPC_NOT_FOUND: &str = "InvalidVpcID.NotFound";

/// The sentence EC2 words for a subnet id it holds no subnet for.
fn subnet_absent_sentence(subnet_id: &str) -> String {
    format!("The subnet ID '{subnet_id}' does not exist")
}

/// The sentence EC2 words for a security group id it holds no group for.
fn group_absent_sentence(group_id: &str) -> String {
    format!("The security group '{group_id}' does not exist")
}

/// The sentence EC2 words for a VPC id it holds no VPC for.
fn vpc_absent_sentence(vpc_id: &str) -> String {
    format!("The vpc ID '{vpc_id}' does not exist")
}

/// The status every refusal relayed here is answered with.
///
/// Each of the codes above is a CLIENT refusal — the request named a resource of the
/// account that does not exist — which EC2 answers HTTP 400 for. The status EC2 ANSWERED
/// is what the clause states whenever the answer carried one; this is what it states for a
/// read that resolved and then reported no resource for one of the ids, where the refusal
/// is of the same class but no refused answer carried a status of its own.
const EC2_CLIENT_REFUSAL_STATUS: u16 = 400;

/// What EC2 answered about one of the three member lists: nothing to refuse the request
/// on, or the refusal to relay.
enum Answer {
    /// EC2 resolved every id the request named — and also the case where there was no EC2
    /// to ask, which grounds no refusal.
    Nothing,
    /// EC2 reports that one of the ids names no such resource of the account: its own
    /// sentence, the error code it answered, the status it answered with, and the id it
    /// gave the call that answered them (`None` when the answer carried none).
    Refused {
        sentence: String,
        code: String,
        status: u16,
        request_id: Option<String>,
    },
}

impl Answer {
    /// The MESSAGE a Device Farm refusal states for this answer, or `None` when the answer
    /// grounds no refusal.
    ///
    /// Device Farm made the read on the caller's behalf, so the refusal relays it WHOLE:
    /// the sentence EC2 worded — which is what names the offending id — followed by the
    /// context of the call behind it, which is how a caller correlates this refusal with
    /// that call, so it is part of the message rather than an addition to it. The id is the
    /// one EC2 gave that call; a per-call correlation value nothing in the request decides,
    /// so where the answer carried none a fresh one is minted rather than left blank.
    fn into_refusal(self) -> Option<String> {
        let Self::Refused {
            sentence,
            code,
            status,
            request_id,
        } = self
        else {
            return None;
        };
        let request_id = request_id
            .unwrap_or_else(::service_common::query_error_envelope::new_request_id);
        Some(format!(
            "{sentence} (Service: {EC2_SERVICE_NAME}; Status Code: {status}; \
             Error Code: {code}; Request ID: {request_id})"
        ))
    }
}

/// The refusal the EC2 resources `config` names earns, as the MESSAGE a Device Farm
/// `ArgumentException` states — or `None` when EC2 resolved every one of them, and also
/// when there is no EC2 to ask.
///
/// Every member of a `vpcConfig` is resolved: a project is attached to the VPC it names,
/// in the subnets it names, under the security groups it names, so each of the three is a
/// reference to a resource that has to exist for the configuration to describe a reachable
/// network. The members are resolved in the order the shape declares them, so one call
/// relays one fault.
pub async fn unresolved_vpc_config(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    config: &VpcConfig,
) -> Option<String> {
    let ec2 = ec2_client(ctx, region);
    if let Some(refusal) = resolve_security_groups(&ec2, &config.security_group_ids)
        .await
        .into_refusal()
    {
        return Some(refusal);
    }
    if let Some(refusal) = resolve_subnets(&ec2, &config.subnet_ids)
        .await
        .into_refusal()
    {
        return Some(refusal);
    }
    resolve_vpc(&ec2, &config.vpc_id).await.into_refusal()
}

/// What EC2 answers about the subnet ids `subnet_ids` names.
async fn resolve_subnets(ec2: &::aws_sdk_ec2::Client, subnet_ids: &[String]) -> Answer {
    if subnet_ids.is_empty() {
        return Answer::Nothing;
    }
    let answer = ec2
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
            // the same thing a refusal does, and about the same id.
            let Some(absent) = subnet_ids.iter().find(|id| !reported.contains(&id.as_str())) else {
                return Answer::Nothing;
            };
            return Answer::Refused {
                sentence: subnet_absent_sentence(absent),
                code: SUBNET_NOT_FOUND.to_string(),
                status: EC2_CLIENT_REFUSAL_STATUS,
                request_id: None,
            };
        }
        Err(error) => error,
    };
    relayed_answer(&error, &|code: &str| code.starts_with(SUBNET_REFUSAL_PREFIX), "the subnets")
}

/// What EC2 answers about the security group ids `group_ids` names.
async fn resolve_security_groups(ec2: &::aws_sdk_ec2::Client, group_ids: &[String]) -> Answer {
    if group_ids.is_empty() {
        return Answer::Nothing;
    }
    let answer = ec2
        .describe_security_groups()
        .set_group_ids(Some(group_ids.to_vec()))
        .send()
        .await;
    let error = match answer {
        Ok(output) => {
            let reported: Vec<&str> = output
                .security_groups()
                .iter()
                .filter_map(::aws_sdk_ec2::types::SecurityGroup::group_id)
                .collect();
            let Some(absent) = group_ids.iter().find(|id| !reported.contains(&id.as_str())) else {
                return Answer::Nothing;
            };
            return Answer::Refused {
                sentence: group_absent_sentence(absent),
                code: GROUP_NOT_FOUND.to_string(),
                status: EC2_CLIENT_REFUSAL_STATUS,
                request_id: None,
            };
        }
        Err(error) => error,
    };
    relayed_answer(&error, &group_is_a_refusal, "the security groups")
}

/// What EC2 answers about the VPC id `vpc_id` names.
async fn resolve_vpc(ec2: &::aws_sdk_ec2::Client, vpc_id: &str) -> Answer {
    if vpc_id.is_empty() {
        return Answer::Nothing;
    }
    let answer = ec2
        .describe_vpcs()
        .vpc_ids(vpc_id.to_string())
        .send()
        .await;
    let error = match answer {
        Ok(output) => {
            let reported = output
                .vpcs()
                .iter()
                .any(|vpc| vpc.vpc_id() == Some(vpc_id));
            if reported {
                return Answer::Nothing;
            }
            return Answer::Refused {
                sentence: vpc_absent_sentence(vpc_id),
                code: VPC_NOT_FOUND.to_string(),
                status: EC2_CLIENT_REFUSAL_STATUS,
                request_id: None,
            };
        }
        Err(error) => error,
    };
    relayed_answer(&error, &|code: &str| code.starts_with(VPC_REFUSAL_PREFIX), "the VPC")
}

/// Whether `code` is one of the security-group existence refusals
/// ([`GROUP_REFUSAL_CODES`]).
fn group_is_a_refusal(code: &str) -> bool {
    GROUP_REFUSAL_CODES.contains(&code)
}

/// The answer an EC2 read that FAILED grounds: the refusal to relay when EC2 reported that
/// one of the ids names no such resource — which `is_a_refusal` decides from the code it
/// answered — and nothing otherwise.
///
/// Anything else is this emulator failing to ASK rather than EC2 reporting a bad id — there
/// is no EC2 mounted, or the answer could not be read — and nothing is what a request is
/// refused on then.
fn relayed_answer<
    E: ::aws_smithy_types::error::metadata::ProvideErrorMetadata
        + ::aws_types::request_id::RequestId
        + ::std::error::Error
        + 'static,
>(
    error: &::aws_smithy_runtime_api::client::result::SdkError<
        E,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
    is_a_refusal: &dyn Fn(&str) -> bool,
    named: &str,
) -> Answer {
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    let code = error.code().unwrap_or_default().to_string();
    if !is_a_refusal(&code) {
        // Nothing was learned, and what a request names stays unresolved: a deployment
        // with no EC2 mounted serves every `vpcConfig` it is given, so the attempt that
        // answered nothing is reported rather than passed over in silence.
        ::tracing::warn!(
            error = %::aws_smithy_types::error::display::DisplayErrorContext(error),
            "{named} a request named could not be resolved against EC2"
        );
        return Answer::Nothing;
    }
    Answer::Refused {
        // EC2 names the offending id in its own sentence, so that sentence is what is
        // relayed. A refusal it worded none for still states what it can: its code.
        sentence: error
            .message()
            .map_or_else(|| code.clone(), str::to_string),
        code,
        // The status and the request id EC2 ANSWERED, read off the answer itself rather
        // than assumed, so the clause reports the call that was really made.
        status: error
            .raw_response()
            .map_or(EC2_CLIENT_REFUSAL_STATUS, |response| {
                response.status().as_u16()
            }),
        request_id: ::aws_types::request_id::RequestId::request_id(error).map(str::to_string),
    }
}
