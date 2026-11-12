// SPDX-License-Identifier: CC0-1.0

//! The VPC endpoint a connector is reachable through, as the service that OWNS it reports it.
//!
//! A connector created with a `VpcEndpointId` is "accessible only through that specific VPC
//! endpoint" (the member's own doc), and a VPC endpoint is not a resource of this service: it
//! is Amazon EC2's. A client builds one with `ec2:CreateVpcEndpoint` and then names it on the
//! create, so EC2 is the ONE answer to whether the account has an endpoint under that id and
//! whether it publishes names a client can resolve it by — and the id a request names is
//! resolved there and nowhere else, over the emulator's in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), so no socket is opened and
//! nothing leaves the process.
//!
//! Nothing is cached and nothing is kept: this service stores the id the create named and
//! asks EC2 afresh, so an endpoint created a moment ago resolves at once. A hop that could
//! not be made at all is evidence of nothing, so it grounds no outcome and says so at `warn`.

use ::smithy_server_runtime::OperationContext;

/// The endpoint prefix the in-process channel resolves Amazon EC2 by.
const ENDPOINT_PREFIX: &str = "ec2";

/// The `DescribeVpcEndpoints` filter that selects endpoints BY ID (`vpc-endpoint-id`, listed
/// by that operation's own `Filters` documentation).
///
/// A filter is what asks the question rather than `VpcEndpointIds`: an id the account has no
/// endpoint for is then an absent row instead of a failure of the whole call, so the answer
/// tells "no such endpoint" apart from "EC2 could not be asked".
const VPC_ENDPOINT_ID_FILTER: &str = "vpc-endpoint-id";

/// What Amazon EC2 answered about the VPC endpoint one request named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolved {
    /// EC2 holds no endpoint of the account under that id.
    Absent,
    /// EC2 holds the endpoint, and it publishes no DNS entry — so there is no name a client
    /// configured against the connector could resolve it by.
    WithoutDnsEntries,
    /// EC2 holds an endpoint that publishes at least one DNS entry — and equally, the answer
    /// when the question could not be put at all, because what the account holds is
    /// established by the owning service's own read and never inferred from a dependency that
    /// did not answer.
    Resolves,
}

impl Resolved {
    /// The `ConnectorStatusReason` a connector reachable only through this endpoint fails
    /// creation with, or `None` when the endpoint is one a client could reach it at.
    pub fn failure_reason(self) -> Option<&'static str> {
        match self {
            Self::Absent => Some(crate::state::REASON_VPC_ENDPOINT_RESOURCE_NOT_FOUND),
            Self::WithoutDnsEntries => {
                Some(crate::state::REASON_VPC_ENDPOINT_DNS_ENTRIES_NOT_FOUND)
            }
            Self::Resolves => None,
        }
    }
}

/// Resolve `vpc_endpoint_id` against the VPC endpoints Amazon EC2 holds for the account in
/// `region`: ONE read of that service, answering both whether the endpoint exists and whether
/// it publishes a name to reach it by.
pub async fn resolve(
    ctx: &OperationContext<'_>,
    region: &str,
    vpc_endpoint_id: &str,
) -> Resolved {
    let by_id = ::aws_sdk_ec2::types::Filter::builder()
        .name(VPC_ENDPOINT_ID_FILTER)
        .values(vpc_endpoint_id)
        .build();
    let answer = client(ctx, region)
        .describe_vpc_endpoints()
        .filters(by_id)
        .send()
        .await;
    let output = match answer {
        Ok(output) => output,
        Err(error) => {
            // At `warn`: nothing is grounded on silence, so an emulator where this hop cannot
            // be made brings up a connector whose `VpcEndpointId` resolves to nothing, and
            // that is worth seeing.
            ::tracing::warn!(
                %region,
                vpc_endpoint = %vpc_endpoint_id,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "Amazon EC2 did not answer whether the VPC endpoint exists"
            );
            return Resolved::Resolves;
        }
    };
    let Some(endpoint) = output
        .vpc_endpoints()
        .iter()
        .find(|endpoint| endpoint.vpc_endpoint_id() == Some(vpc_endpoint_id))
    else {
        return Resolved::Absent;
    };
    // "(Interface endpoint) The DNS entries for the endpoint" (`VpcEndpoint$DnsEntries`): a
    // connector published under a VPC endpoint's name is reached through one of those names,
    // so an endpoint carrying none is an endpoint nothing resolves to.
    if endpoint.dns_entries().is_empty() {
        return Resolved::WithoutDnsEntries;
    }
    Resolved::Resolves
}

/// The Amazon EC2 client this service reads that service through: the AWS SDK driven over the
/// emulator's own in-process transport, signing in the emulator's name. One seam, used only by
/// the resolution above.
///
/// `region` is the Region the caller signed for, which this service has already read as a
/// Region NAME ([`crate::state::request_region`]) rather than as free text, so the endpoint
/// URL below names the peer service this emulator asks and never an authority the caller
/// chose. That URL is only the address the in-process channel routes on; the credentials are
/// the emulator's own, used to sign this hop, and are never returned or logged.
fn client(ctx: &OperationContext<'_>, region: &str) -> ::aws_sdk_ec2::Client {
    let config = ::aws_sdk_ec2::Config::builder()
        .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_ec2::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://{ENDPOINT_PREFIX}.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client is
        // not given. The same goes for caching the identity: the credentials are handed over
        // directly.
        .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_ec2::Client::from_conf(config)
}
