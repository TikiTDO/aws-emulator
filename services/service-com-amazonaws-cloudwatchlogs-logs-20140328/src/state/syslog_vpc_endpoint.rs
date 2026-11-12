// SPDX-License-Identifier: CC0-1.0

//! The VPC endpoint a syslog configuration ingests through, resolved where it is held.
//!
//! `PutSyslogConfiguration` "enables ingestion of syslog data through the specified VPC
//! endpoint into the log group", so the `vpcEndpointId` a put carries names a resource the
//! configuration depends on. A VPC endpoint is not a resource of this service: it is Amazon
//! EC2's, built with `ec2:CreateVpcEndpoint`, so EC2 is the ONE answer to "does the account
//! have an endpoint under this id" and an identifier a request names is resolved there and
//! nowhere else. A put naming an endpoint the account does not have is refused before
//! anything is stored, because the ingestion it would configure could never happen.
//!
//! EC2 is reached the way the framework documents a handler reaching another emulated service
//! ([`smithy_server_runtime::OperationContext::callback`]): an AWS SDK client over the
//! IN-PROCESS callback's HTTP client and credentials ([`super::callback_client`]) — the
//! callback is this module's only transport, taken as a parameter, so nothing here opens a
//! socket or leaves the host, and the only caller-supplied value that reaches the client's
//! authority is the signing Region, narrowed first
//! ([`service_common::arn::region_label`]).
//!
//! Nothing is cached: an endpoint created a moment ago resolves at once, and one that was
//! deleted stops resolving at once. An answer that is not EC2 stating which endpoints the
//! account has — an emulator with no EC2 mounted beside this service, any other failure to
//! ask — establishes NOTHING about the identifier, so it refuses nothing: a refusal has to be
//! grounded in the answer EC2 gave.

use super::callback_client::callback_client;

/// The `DescribeVpcEndpoints` filter that selects endpoints BY ID (`vpc-endpoint-id`, listed
/// by that operation's own `Filters` documentation).
///
/// A filter is what asks the question rather than `VpcEndpointIds`: an identifier the account
/// has no endpoint for is then an absent row instead of a failure of the whole call, so the
/// answer distinguishes "no such endpoint" from "EC2 could not be asked".
const VPC_ENDPOINT_ID_FILTER: &str = "vpc-endpoint-id";

/// The refusal CloudWatch Logs answers when the `vpcEndpointId` a syslog configuration names
/// does not resolve to a VPC endpoint the account has.
///
/// The service draws no distinction between an id that is not an endpoint of this account and
/// one it may not read — both leave it unable to confirm the ingestion path — so one sentence
/// states every possibility, with the requested `vpcEndpointId` as its only variable part.
#[must_use]
pub fn failed_validation_message(vpc_endpoint_id: &str) -> String {
    format!(
        "VPC endpoint {vpc_endpoint_id} failed validation. Verify the VPC endpoint ID is \
         correct, belongs to your account, and you have the required EC2 permissions."
    )
}

/// The refusal CloudWatch Logs answers when the endpoint a `DeleteSyslogConfiguration` names
/// ingests into no log group of that name — the group holds no configuration for it, so there
/// is no ingestion to take away.
///
/// Both variable parts come from the request as resolved against the store: the requested
/// `vpcEndpointId`, and the NAME of the log group it was asked about (an identifier that is an
/// ARN resolves to that name), so a caller detaching endpoints in a loop can attribute the
/// refusal to one pair instead of guessing which of its requests failed.
#[must_use]
pub fn not_associated_message(vpc_endpoint_id: &str, log_group_name: &str) -> String {
    format!("VPC endpoint {vpc_endpoint_id} is not associated with log group {log_group_name}")
}

/// `Some(message)` when EC2 answered that the account has no VPC endpoint under
/// `vpc_endpoint_id` in `region`, and `None` when it has one — or when EC2 did not answer at
/// all, which establishes nothing (see the module docs).
///
/// The transport is the parameter: the ONLY way to reach EC2 from here is the in-process
/// callback a handler is given ([`EmbeddedEmulator`]), so no ambient or environment-resolved
/// configuration can turn this hop into a real connection, and a call site shows which
/// transport it hands over.
pub async fn refusal_for(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    vpc_endpoint_id: &str,
) -> Option<String> {
    let region =
        ::service_common::arn::region_label(region).unwrap_or(crate::state::DEFAULT_REGION);
    let client = callback_client!(aws_sdk_ec2, callback, "ec2", region);
    let by_id = ::aws_sdk_ec2::types::Filter::builder()
        .name(VPC_ENDPOINT_ID_FILTER)
        .values(vpc_endpoint_id)
        .build();
    match client.describe_vpc_endpoints().filters(by_id).send().await {
        Ok(answer) => {
            let held = answer
                .vpc_endpoints()
                .iter()
                .any(|endpoint| endpoint.vpc_endpoint_id() == Some(vpc_endpoint_id));
            (!held).then(|| failed_validation_message(vpc_endpoint_id))
        }
        // Not EC2 reporting which endpoints the account has, so nothing is established about
        // the identifier and nothing is refused — which means a put is admitted without its
        // endpoint having been resolved, so this is worth seeing at `warn`. The caller's own
        // value is rendered in its escaped `Debug` form, so an identifier carrying a newline
        // cannot forge a log line.
        Err(error) => {
            ::tracing::warn!(
                vpc_endpoint = ?vpc_endpoint_id,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "EC2 did not state which VPC endpoints the account has, so the endpoint this \
                 request names is not judged against them"
            );
            None
        }
    }
}
