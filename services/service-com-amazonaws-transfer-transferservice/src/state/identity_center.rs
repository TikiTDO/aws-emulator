// SPDX-License-Identifier: CC0-1.0

//! The IAM Identity Center dependency a web app has: the instance its application is
//! registered in, and the refusal the registration is denied with.
//!
//! A Transfer Family web app authenticates through IAM Identity Center:
//! `CreateWebApp.IdentityProviderDetails` names the instance, and
//! `DescribedIdentityCenterConfig.ApplicationArn` is "the ARN for the IAM Identity
//! Center application: this value is set automatically when you create your web app" —
//! so the create performs `sso:CreateApplication` on the caller's behalf, in that
//! instance. The application is a resource of the INSTANCE, so an instance the account
//! does not hold has no application namespace to register one in and the dependent call
//! is denied; Transfer answers that denial on its own modelled `AccessDeniedException`
//! channel, with the standard IAM authorization-failure sentence the denied call
//! carried, and creates nothing.
//!
//! WHICH instances the account holds is the owning service's own rule, asked here
//! through its read-only reading of it
//! ([`::service_com_amazonaws_ssoadmin_swbexternalservice::state::ComAmazonawsSsoadminSwbexternalserviceState::holds_instance`]),
//! so nothing about that rule is restated in this crate and a caller that enables (or
//! deletes) its instance changes the next answer. Asking is a READ: it enables no
//! instance and decides nothing for the Identity Center service.
//!
//! Nothing is refused on an assumption: an emulator with no Identity Center mounted to
//! ask answers nothing about the account's instances, so it refuses nothing — the same
//! reading [`super::authorization`] takes of a store it cannot read.

use super::*;

/// The IAM Identity Center action a web-app create performs on the caller's behalf, as
/// IAM names it.
pub const SSO_CREATE_APPLICATION: &str = "sso:CreateApplication";

/// The partition an Identity Center resource is named under when the instance the
/// request names is not a well-formed ARN to read one off. Every ARN this emulator
/// emits carries it.
const DEFAULT_PARTITION: &str = "aws";

/// The IAM resource an `sso:CreateApplication` is evaluated against: the APPLICATION
/// namespace of one Identity Center instance,
/// `arn:<partition>:sso::*:application/<instance id>/*`.
///
/// An Identity Center application is a resource of its instance, so the instance's id
/// is part of the resource's name; the account and the application id are the IAM
/// wildcards the authorization is evaluated over, since the application has no id until
/// the call that would create it succeeds. The instance id is read off the ARN the
/// request named (`InstanceArn`'s modelled pattern
/// `^arn:[\w-]+:sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}$` puts it after the
/// `:instance/` segment), so nothing here is derived from a recording.
pub fn identity_center_application_resource(instance_arn: &str) -> String {
    let partition = ::service_common::arn::parse(instance_arn)
        .map(|arn| arn.partition)
        .unwrap_or(DEFAULT_PARTITION);
    let instance = instance_arn
        .split_once(":instance/")
        .map(|(_, instance)| instance)
        .unwrap_or(instance_arn);
    format!("arn:{partition}:sso::*:application/{instance}/*")
}

/// The sentence IAM states an authorization failure with when NO resource-based policy
/// on the resource allows the action, from the ONE shared statement of it
/// ([`::service_common::iam_authorization::not_authorized_by_resource_policy`]) rather
/// than a copy: the sentence is one rule, so two services that reach the same decision
/// word it the same way.
///
/// This is the form a cross-service call carries: the action is performed by the
/// service on the caller's behalf, so the decision is taken on the OTHER service's
/// resource, and the resource is what has to allow it. Every value in it comes from the
/// request and the account's own state.
pub fn not_authorized_without_resource_policy(
    principal: &str,
    action: &str,
    resource: &str,
) -> String {
    ::service_common::iam_authorization::not_authorized_by_resource_policy(
        principal, action, resource,
    )
}

/// Whether the account's Identity Center ANSWERED that `instance_arn` names no instance
/// of this account.
///
/// `false` — refuse nothing — when nothing is KNOWN about the account's instances: an
/// emulator the Identity Center service is not mounted on cannot show the instance to
/// be absent.
pub fn identity_center_instance_is_absent<Eng: Send + Sync + 'static>(
    engine: &Eng,
    instance_arn: &str,
) -> bool {
    use ::service_com_amazonaws_ssoadmin_swbexternalservice::state as identity_center;

    let Some(state) = identity_center::service_state(engine) else {
        return false;
    };
    let held = state.lock().holds_instance(instance_arn);
    !held
}

/// The refusal a web-app create gets when the `sso:CreateApplication` it performs on
/// the caller's behalf is denied, or `None` when nothing denies it.
///
/// The one condition this service can SHOW: the instance the request names is not one
/// the account's Identity Center holds, so there is no application namespace to create
/// the application in. `instance_arn` is `None` for a request that names no instance at
/// all — there is then no dependent call to make and nothing to deny.
pub fn sso_create_application_denied<Eng: Send + Sync + 'static>(
    engine: &Eng,
    parts: &::http::request::Parts,
    instance_arn: Option<&str>,
) -> Option<AccessDeniedException> {
    let instance_arn = instance_arn?;
    if !identity_center_instance_is_absent(engine, instance_arn) {
        return None;
    }
    Some(AccessDeniedException {
        message: Some(not_authorized_without_resource_policy(
            &authorization::calling_principal(engine, parts),
            SSO_CREATE_APPLICATION,
            &identity_center_application_resource(instance_arn),
        )),
    })
}
