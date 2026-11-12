// SPDX-License-Identifier: CC0-1.0

//! The DISCOVERY a registration runs on the Amazon EC2 instances it named.
//!
//! Registering an SAP application is asynchronous because the service DISCOVERS the
//! application: it runs a Systems Manager document on each Amazon EC2 instance the
//! registration named, and what that run finds is the SAP system the registration ends up
//! managing. So the outcome of a registration is the outcome of that run, and a run that
//! could not be carried out at all is a registration that FAILED
//! ([`crate::state::DISCOVERY_STATUS_REGISTRATION_FAILED`]), not one that found nothing.
//!
//! An instance is EC2's resource, not this service's, so whether a document can be run on
//! one is EC2's to answer: this asks it, over the one client every hop into EC2 goes
//! through ([`super::ec2::client`]), and reports success only for an instance EC2 itself
//! answers for and reports running. Nothing here invents an instance, and nothing is
//! concluded from the id's form: an id EC2 holds no record of, and one it will not read as
//! an instance id at all, are alike in that no document reached an instance
//! ([`UNRESOLVED_INSTANCE_CODE_PREFIX`]).
//!
//! Nothing is invented when EC2 cannot be ASKED either (no EC2 mounted beside this
//! service, or a hop that reached no service that could answer): the hop is logged at
//! `warn` and the discovery is not judged by it, because this service observed nothing
//! about the instance and must not report a discovery result it did not get.

use ::aws_sdk_ec2::error::ProvideErrorMetadata;

use super::{ec2_instance_arn, new_operation_id};

/// The family of error codes the owner of an instance answers about an INSTANCE ID it
/// cannot resolve to an instance of the account: `InvalidInstanceID.NotFound` for a
/// well-formed id it holds nothing for, `InvalidInstanceID.Malformed` for a value it does
/// not read as an instance id at all.
///
/// A refusal in this family is EC2's answer that there is no such instance, which is what
/// makes it a discovery that could not be carried out; any other failure of the hop says
/// nothing about the instance ([`run_document`] leaves the discovery unjudged for those).
const UNRESOLVED_INSTANCE_CODE_PREFIX: &str = "InvalidInstanceID";

/// The IAM action the discovery of a registration needs on the instances it runs on:
/// running a Systems Manager document on a node is a `SendCommand`.
const DISCOVERY_ACTION: &str = "ssm:SendCommand";

/// The identity AWS Systems Manager for SAP discovers an application as: the service-linked
/// role it assumes in the account, and the session name it assumes it under, as the ARN of
/// the assumed role in a discovery refusal names them.
const DISCOVERY_IDENTITY: &str = "assumed-role/AWSServiceRoleForAWSSSMForSAP/ssm-sap";

/// The ARN namespace of an assumed-role identity, which is what the refusal names the
/// caller by rather than the role resource itself.
const STS_ARN_SERVICE: &str = "sts";

/// How many attempts the inner call that carries the document took: ONE, the call this
/// hop made. It is reported because the refusal a client reads is the inner SDK's own,
/// which states the attempt it failed on; the number is per-run information a client must
/// not depend on.
const DISCOVERY_ATTEMPT_COUNT: u32 = 1;

/// Whether the discovery of a registration on `instance_ids` can be carried out, and the
/// refusal it ran into when it cannot.
///
/// `None` is a discovery that can complete: every instance named is one the owner of
/// instances answers for and reports ready to take a document. Otherwise the refusal
/// [`document_refusal`] words for the FIRST instance no document could be run on — the
/// registration converges to a failed discovery as a whole, so the run stops at the
/// instance that could not take it.
///
/// Each instance is asked for ON ITS OWN, so the walk is as long as the list it is handed:
/// the instances one registration named, which the model bounds at one
/// (`InstanceList` is `@length(min: 1, max: 1)`).
pub async fn run_document(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    instance_ids: &[String],
) -> Option<String> {
    if instance_ids.is_empty() {
        return None;
    }
    let client = super::ec2::client(ctx, region);
    for instance_id in instance_ids {
        match client
            .describe_instances()
            .instance_ids(instance_id)
            .send()
            .await
        {
            Ok(described) => match reported_state(&described, instance_id) {
                // The owner reports the instance ready to run a document, so the
                // discovery of this instance can be carried out.
                Some(::aws_sdk_ec2::types::InstanceStateName::Running) => continue,
                Some(state) => {
                    // The owner answered for the instance and it is in no state to run
                    // one: an agent runs on a running instance, so no document reached it.
                    ::tracing::warn!(
                        %region,
                        %instance_id,
                        state = state.as_str(),
                        "the Amazon EC2 instance of an SAP registration is in no state to \
                         run the discovery document"
                    );
                    return Some(document_refusal(region, instance_id));
                }
                None => {
                    // A describe of ONE instance either reports it or refuses the id, so a
                    // reply that reports nothing about it is no answer about the instance:
                    // this service observed nothing, and invents no discovery result.
                    ::tracing::warn!(
                        %region,
                        %instance_id,
                        "the owner of the Amazon EC2 instance of an SAP registration \
                         reported nothing about it"
                    );
                }
            },
            Err(error) => {
                // Only a refusal ABOUT THE INSTANCE is an answer this can conclude
                // anything from: the owner resolved the id to no instance of the account,
                // so no document reached one. Any other failure — no EC2 mounted beside
                // this service, a hop that reached no service that could answer, a refusal
                // about something else — leaves this service having observed nothing about
                // the instance, so no discovery result is invented from it.
                let unresolved = error
                    .code()
                    .is_some_and(|code| code.starts_with(UNRESOLVED_INSTANCE_CODE_PREFIX));
                // Visible at default verbosity either way: a registration whose discovery
                // could not reach its instance is exactly what this reports, so neither
                // the refusal nor the unanswered hop is silent.
                ::tracing::warn!(
                    %region,
                    %instance_id,
                    unresolved,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the Amazon EC2 instance of an SAP registration could not be described"
                );
                if unresolved {
                    return Some(document_refusal(region, instance_id));
                }
            }
        }
    }
    None
}

/// The `InstanceState$Name` `described` reports for `instance_id`, and `None` when it
/// reports no state for it at all.
///
/// A document reaches an instance through the agent running ON it, so only an instance the
/// owner reports as `running` can run one. The state is read out of EC2's own reply rather
/// than from anything this service holds, so what a registration concludes about an
/// instance is backed by the service that owns it.
fn reported_state(
    described: &::aws_sdk_ec2::operation::describe_instances::DescribeInstancesOutput,
    instance_id: &str,
) -> Option<::aws_sdk_ec2::types::InstanceStateName> {
    described
        .reservations()
        .iter()
        .flat_map(::aws_sdk_ec2::types::Reservation::instances)
        .filter(|instance| instance.instance_id() == Some(instance_id))
        .find_map(|instance| {
            instance
                .state()
                .and_then(::aws_sdk_ec2::types::InstanceState::name)
                .cloned()
        })
}

/// How a failed registration explains itself: the refusal the inner Systems Manager call
/// that carries the discovery document to `instance_id` answered with, relayed as this
/// service relays it.
///
/// This is what the registration's own operation record reports in `StatusMessage`
/// ([`crate::state::StoredOperation::terminal_status_message`]), and it is the service's
/// own wording for this class of failure verbatim, as the `ssm-sap` registration
/// recordings show it: the identity the service discovers an application as, the action it
/// needs on the instance and the instance's own ARN, around the inner service's error.
/// The account and the Region come from the signing context the call was made in and the
/// instance from the request; the request id is the inner call's own, so it is per-run
/// information a client must not depend on, exactly as the attempt count is.
pub fn document_refusal(region: &str, instance_id: &str) -> String {
    let identity = ::engine::emulator_arn(STS_ARN_SERVICE, "", DISCOVERY_IDENTITY);
    format!(
        "Error executing SSM document - AccessDeniedException User: {identity} is not \
         authorized to perform: {DISCOVERY_ACTION} on resource: {} because no \
         identity-based policy allows the {DISCOVERY_ACTION} action (Service: Ssm, Status \
         Code: 400, Request ID: {}) (SDK Attempt Count: {DISCOVERY_ATTEMPT_COUNT})",
        ec2_instance_arn(region, instance_id),
        new_operation_id(),
    )
}
