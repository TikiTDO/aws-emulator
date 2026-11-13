// SPDX-License-Identifier: CC0-1.0

//! Whether CloudWatch Logs can ASSUME the role a subscription delivery is configured under.
//!
//! A put that configures a delivery to a streaming destination names the role the service
//! delivers under — `PutSubscriptionFilter$roleArn` is "an IAM role that grants CloudWatch Logs
//! permissions to deliver ingested log events to the destination stream" and
//! `PutDestination$roleArn` the same for the destination's target — and CloudWatch Logs
//! VALIDATES that wiring while it configures it, by assuming the role and delivering a test
//! message to the stream ([`super::subscription_delivery::health_check`] is this emulator's
//! test delivery). A role whose trust policy does not let this service's own principal
//! ([`super::subscription_delivery::SERVICE_PRINCIPAL`]) assume it cannot be assumed, so no
//! test message can be delivered under it and the put is refused rather than leaving a
//! configuration that can never deliver anything.
//!
//! The trust question itself is IAM's, asked of IAM the one way the tree asks it
//! ([`service_common::iam_role::held_role_is_unassumable`]), so a refusal is grounded in what
//! IAM answered about the role: a role this account's IAM holds nothing under refuses nothing,
//! a delivery role being routinely provisioned out of band.

/// The refusal for a configuration whose delivery role CloudWatch Logs cannot assume, or `None`
/// when the role grounds no refusal — IAM reports a trust policy that admits this service, it
/// holds no such role, it could not be asked, or the destination is not one written to under a
/// role at all.
///
/// The sentence is the service's own for that error class: the test message could not be
/// delivered to the named kind of stream, and the stream's state is what to check. Its variable
/// part is the KIND of stream the configuration names, read with the one reader of that question
/// ([`destination_label`]); nothing of the request's own values is interpolated.
pub async fn unassumable_role_message(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
    destination_arn: &str,
) -> Option<String> {
    let label = destination_label(destination_arn)?;
    ::service_common::iam_role::held_role_is_unassumable(
        ctx,
        region,
        role_arn,
        super::subscription_delivery::SERVICE_PRINCIPAL,
    )
    .await
    .then(|| {
        format!(
            "Could not deliver test message to specified {label} stream. Check if the given \
             {label} stream is in ACTIVE state."
        )
    })
}

/// The product name of the stream `destination_arn` names, or `None` for a destination
/// CloudWatch Logs does not assume a caller-named role to write to.
///
/// Both halves are the readers this service already has: which destinations are written to UNDER
/// a caller-named role at all ([`super::subscription_role_required_vendor`] — the two streaming
/// vendors, since a Lambda function is invoked directly and a logical destination carries the
/// role it was created with), and what the kind of destination an ARN names is called
/// ([`super::subscription_delivery::destination_type`]).
fn destination_label(destination_arn: &str) -> Option<&'static str> {
    super::subscription_role_required_vendor(destination_arn)?;
    super::subscription_delivery::destination_type(destination_arn)
}
